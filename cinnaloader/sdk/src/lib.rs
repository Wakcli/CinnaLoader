//! The versioned C ABI between the Cinnaloader loader and a mod cdylib.
//!
//! A mod implements [`Mod`] and calls [`export_mod!`]. That generates the `extern "C"`
//! entry points the loader resolves by name. The loader hands each mod a [`HostApi`]
//! table of function pointers; nothing else crosses the boundary, so the ABI stays
//! stable without a shared runtime.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! #[derive(Default)]
//! struct Sprint;
//!
//! impl Mod for Sprint {
//!     fn load(&mut self) {
//!         feature("sprint", "Sprint", "Movement", ON);
//!         slider("speed", "Sprint speed", "Movement", 1.2, 0.1..=5.0, 0.1);
//!         keybind("panic", "Panic key", "Movement", "P");
//!     }
//!
//!     fn tick(&mut self, _dt_millis: u32) {
//!         if enabled("sprint") {
//!             // ... let the local player sprint, at `number("speed")`
//!         }
//!     }
//!
//!     fn toggle(&mut self, id: &str, on: bool) {
//!         info!("{id} is now {on}");
//!     }
//!
//!     fn keybind(&mut self, id: &str, down: bool) {
//!         if id == "panic" && down {
//!             set_enabled("sprint", false);
//!         }
//!     }
//! }
//!
//! export_mod!(Sprint);
//! ```

use std::{
    ffi::{c_char, CStr, CString},
    ops::RangeInclusive,
    sync::OnceLock,
};

pub mod blocks;
pub mod capability;
pub mod compat;
pub mod components;
pub mod config;
pub mod deps;
pub mod diagnostics;
pub mod effects;
pub mod entities;
pub mod events;
pub mod fs;
pub mod game;
pub mod i18n;
pub mod inventory;
pub mod items;
pub mod lifecycle;
pub mod log;
pub mod metadata;
pub mod modinfo;
pub mod mods;
pub mod permissions;
pub mod physics;
pub mod player;
pub mod raycast;
pub mod resources;
pub mod storage;
pub mod world;
pub mod tasks;
pub mod version;

/// Bumped whenever `HostApi` or any entry-point signature changes. A mod whose
/// [`Mod`] crate declared a different value is refused at load.
///
/// 8: the graph and `mods_command` slots, and a manifest declared before load.
pub const ABI_VERSION: u32 = 8;

/// One lock for the tests of every module that touches a process-wide registry, so two
/// tests never drain each other's queue.
#[cfg(test)]
pub(crate) mod test_lock {
    use std::sync::{Mutex, MutexGuard};

    pub fn serial() -> MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }
}

/// The capability system, under the name the wishlist uses.
pub use capability as capabilities;

/// Readable spelling for a toggle's `default`.
pub const ON: bool = true;
/// The other one.
pub const OFF: bool = false;

pub mod level {
    pub const ERROR: u32 = 0;
    pub const WARN: u32 = 1;
    pub const INFO: u32 = 2;
}

/// Host services a mod may call. Field order is part of the ABI.
#[repr(C)]
pub struct HostApi {
    pub abi_version: u32,
    /// Renders one line through the loader's log. Never null while `load` runs.
    pub log: Option<unsafe extern "C" fn(level: u32, message: *const c_char)>,
    /// UTF-8 NUL-terminated directory the mods were loaded from.
    pub mods_dir: *const c_char,
    /// Adds one line to the loader's on-screen overlay.
    pub overlay_line: Option<unsafe extern "C" fn(message: *const c_char)>,
    /// Toggles the loader's ClickGUI.
    pub toggle_gui: Option<unsafe extern "C" fn()>,
    /// This mod's own name, as the loader knows it. Setting ids are scoped by it.
    pub mod_name: *const c_char,
    /// Registers a ClickGUI toggle and returns 1, or 0 when the id is taken.
    pub feature: Option<
        unsafe extern "C" fn(
            id: *const c_char,
            label: *const c_char,
            category: *const c_char,
            default_on: u32,
        ) -> i32,
    >,
    /// Registers a numeric slider.
    pub slider: Option<
        unsafe extern "C" fn(
            id: *const c_char,
            label: *const c_char,
            category: *const c_char,
            value: f64,
            min: f64,
            max: f64,
            step: f64,
        ) -> i32,
    >,
    /// Registers a dropdown; `options` is newline-separated.
    pub choice: Option<
        unsafe extern "C" fn(
            id: *const c_char,
            label: *const c_char,
            category: *const c_char,
            options: *const c_char,
            default_index: u32,
        ) -> i32,
    >,
    /// Registers a rebindable key.
    pub keybind: Option<
        unsafe extern "C" fn(
            id: *const c_char,
            label: *const c_char,
            category: *const c_char,
            default_key: *const c_char,
        ) -> i32,
    >,
    /// Registers a one-shot button.
    pub button: Option<
        unsafe extern "C" fn(
            id: *const c_char,
            label: *const c_char,
            category: *const c_char,
        ) -> i32,
    >,
    /// A toggle's state, or -1 when no such toggle is registered.
    pub enabled: Option<unsafe extern "C" fn(id: *const c_char) -> i32>,
    /// Sets a toggle and returns 1.
    pub set_enabled: Option<unsafe extern "C" fn(id: *const c_char, on: u32) -> i32>,
    /// A slider's value; a finite `value` sets it first.
    pub number: Option<unsafe extern "C" fn(id: *const c_char, value: f64) -> f64>,
    /// Sets a slider's value and returns 1.
    pub set_number: Option<unsafe extern "C" fn(id: *const c_char, value: f64) -> i32>,
    /// A dropdown's selected index, or -1.
    pub choice_index: Option<unsafe extern "C" fn(id: *const c_char) -> i32>,
    /// Writes a keybind's key name into `out`.
    pub key_name:
        Option<unsafe extern "C" fn(id: *const c_char, out: *mut c_char, capacity: usize) -> i32>,
    /// Writes the next queued event as `kind\tid\tvalue`; 0 when none is pending.
    pub take_event: Option<unsafe extern "C" fn(out: *mut c_char, capacity: usize) -> i32>,
    /// Reads a mod-private config value.
    pub config:
        Option<unsafe extern "C" fn(key: *const c_char, out: *mut c_char, capacity: usize) -> i32>,
    /// Stores a mod-private config value.
    pub set_config: Option<unsafe extern "C" fn(key: *const c_char, value: *const c_char) -> i32>,
    /// Declares this mod's manifest, one `key=value` per line. Returns 1 when accepted.
    pub declare: Option<unsafe extern "C" fn(text: *const c_char) -> i32>,
    /// Writes the loader's view of this mod, one `key=value` per line.
    pub report: Option<unsafe extern "C" fn(out: *mut c_char, capacity: usize) -> i32>,
    /// Writes the registry and dependency graph, one `key=value` per line.
    pub graph: Option<unsafe extern "C" fn(out: *mut c_char, capacity: usize) -> i32>,
    /// Runs one command on another mod: 0 enable, 1 disable, 2 is enabled, 3 is loaded.
    pub mods_command: Option<unsafe extern "C" fn(verb: u32, id: *const c_char) -> i32>,
}

/// A rate the loader's tick can drive: `Timer::every(1000)` fires about once a second.
#[derive(Debug, Clone, Copy)]
pub struct Timer {
    remaining: u64,
    interval: Option<u64>,
}

impl Default for Timer {
    /// Unset: never fires until [`Timer::after`] or [`Timer::every`] replaces it.
    fn default() -> Self {
        Self {
            remaining: u64::MAX,
            interval: None,
        }
    }
}

impl Timer {
    /// Fires once, `millis` from now.
    #[must_use]
    pub fn after(millis: u64) -> Self {
        Self {
            remaining: millis.max(1),
            interval: None,
        }
    }

    /// Fires every `millis`, starting one interval from now.
    #[must_use]
    pub fn every(millis: u64) -> Self {
        let interval = millis.max(1);
        Self {
            remaining: interval,
            interval: Some(interval),
        }
    }

    /// Advances by one tick and returns whether the timer fired.
    pub fn ready(&mut self, dt_millis: u32) -> bool {
        let dt = u64::from(dt_millis);
        if self.remaining > dt {
            self.remaining -= dt;
            return false;
        }
        match self.interval {
            Some(interval) => {
                self.remaining = interval;
                true
            }
            None if self.remaining != u64::MAX => {
                self.remaining = u64::MAX;
                true
            }
            None => false,
        }
    }
}

macro_rules! slot {
    ($name:ident, $ty:ty) => {
        static $name: OnceLock<$ty> = OnceLock::new();
    };
}

slot!(LOG, unsafe extern "C" fn(u32, *const c_char));
slot!(OVERLAY_LINE, unsafe extern "C" fn(*const c_char));
slot!(TOGGLE_GUI, unsafe extern "C" fn());
slot!(MOD_NAME, String);
slot!(MODS_DIR_PATH, String);

slot!(
    REGISTER_FEATURE,
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, u32) -> i32
);
slot!(
    REGISTER_SLIDER,
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, f64, f64, f64, f64) -> i32
);
slot!(
    REGISTER_CHOICE,
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char, u32) -> i32
);
slot!(
    REGISTER_KEYBIND,
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *const c_char) -> i32
);
slot!(
    REGISTER_BUTTON,
    unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> i32
);
slot!(READ_ENABLED, unsafe extern "C" fn(*const c_char) -> i32);
slot!(WRITE_ENABLED, unsafe extern "C" fn(*const c_char, u32) -> i32);
slot!(READ_NUMBER, unsafe extern "C" fn(*const c_char, f64) -> f64);
slot!(WRITE_NUMBER, unsafe extern "C" fn(*const c_char, f64) -> i32);
slot!(READ_CHOICE, unsafe extern "C" fn(*const c_char) -> i32);
slot!(
    READ_KEY_NAME,
    unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32
);
slot!(
    TAKE_EVENT,
    unsafe extern "C" fn(*mut c_char, usize) -> i32
);
slot!(
    READ_CONFIG,
    unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32
);
slot!(
    WRITE_CONFIG,
    unsafe extern "C" fn(*const c_char, *const c_char) -> i32
);
slot!(DECLARE, unsafe extern "C" fn(*const c_char) -> i32);
slot!(GRAPH, unsafe extern "C" fn(*mut c_char, usize) -> i32);
slot!(MODS_COMMAND, unsafe extern "C" fn(u32, *const c_char) -> i32);
slot!(REPORT, unsafe extern "C" fn(*mut c_char, usize) -> i32);

/// Writes one line through the loader, falling back to stderr before `load`.
pub fn log(level: u32, message: &str) {
    let Some(log) = LOG.get() else {
        eprintln!("[cinnaloader-pending/{}] {message}", message_level(level));
        return;
    };
    if let Ok(c) = CString::new(message) {
        // SAFETY: `log` is a loader-owned function pointer valid for the process.
        unsafe { log(level, c.as_ptr()) };
    }
}

/// [`log`] at [`level::INFO`].
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::log($crate::level::INFO, &::std::format!($($arg)*)) };
}

/// [`log`] at [`level::WARN`].
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::log($crate::level::WARN, &::std::format!($($arg)*)) };
}

/// [`log`] at [`level::ERROR`].
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::log($crate::level::ERROR, &::std::format!($($arg)*)) };
}

/// Declares a mod the quick way: a name, a version and an author.
///
/// ```
/// # use cinnaloader::meta;
/// meta!("Hello mod", env!("CARGO_PKG_VERSION"), "you");
/// ```
#[macro_export]
macro_rules! meta {
    ($name:expr, $version:expr, $authors:expr) => {
        $crate::metadata::declare(
            $crate::modinfo::ModManifest::new(env!("CARGO_PKG_NAME"), $name, $version)
                .author($authors),
        )
    };
}

/// Hands a declared manifest to the loader. Called by [`metadata::declare`].
pub(crate) fn install_manifest(manifest: &modinfo::ModManifest) -> bool {
    let Some(declare) = DECLARE.get() else {
        return true;
    };
    let Ok(text) = CString::new(manifest.encode()) else {
        return false;
    };
    // SAFETY: `declare` is a loader-owned function pointer, and `text` outlives the call.
    unsafe { declare(text.as_ptr()) > 0 }
}

/// The loader's registry and dependency graph, or `None` when no loader is present.
pub(crate) fn read_graph() -> Option<String> {
    read_blob(*GRAPH.get()?, 16 * 1024)
}

/// The loader's command entry point, for the `mods` module.
pub(crate) fn mods_command() -> Option<unsafe extern "C" fn(u32, *const c_char) -> i32> {
    MODS_COMMAND.get().copied()
}

/// Reads a loader-written blob into a string.
fn read_blob(
    read: unsafe extern "C" fn(*mut c_char, usize) -> i32,
    capacity: usize,
) -> Option<String> {
    let mut buffer = vec![0 as c_char; capacity];
    // SAFETY: `read` writes at most `buffer.len()` bytes, NUL-terminated.
    if unsafe { read(buffer.as_mut_ptr(), buffer.len()) } <= 0 {
        return None;
    }
    // SAFETY: the loader wrote a NUL-terminated blob.
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// The loader's view of this mod, or `None` when no loader is present.
pub(crate) fn read_report() -> Option<String> {
    let report = REPORT.get()?;
    let mut buffer = vec![0 as c_char; modinfo::MAX_BYTES * 2];
    // SAFETY: `report` writes at most `buffer.len()` bytes, NUL-terminated.
    if unsafe { report(buffer.as_mut_ptr(), buffer.len()) } <= 0 {
        return None;
    }
    // SAFETY: the loader wrote a NUL-terminated report.
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// The directory the mods were loaded from, once `load` has run.
#[must_use]
pub fn mods_dir() -> Option<&'static str> {
    MODS_DIR_PATH.get().map(String::as_str)
}

/// Adds one line to the loader's on-screen overlay (no-op before `load`).
pub fn overlay_line(line: &str) {
    let Some(push) = OVERLAY_LINE.get() else {
        return;
    };
    if let Ok(c) = CString::new(line) {
        // SAFETY: `push` is a loader-owned function pointer valid for the process.
        unsafe { push(c.as_ptr()) };
    }
}

/// Toggles the loader's ClickGUI (no-op before `load`).
pub fn toggle_gui() {
    if let Some(toggle) = TOGGLE_GUI.get() {
        // SAFETY: `toggle` is a loader-owned function pointer valid for the process.
        unsafe { toggle() };
    }
}

/// Registers a toggle the user drives from the loader's ClickGUI, under `category`.
///
/// Call it from [`Mod::load`]. Ids are scoped to your mod, so two mods may both use
/// `"sprint"`. Read it with [`enabled`]; react in [`Mod::toggle`].
pub fn feature(id: &str, label: &str, category: &str, default: bool) -> bool {
    let Some(register) = REGISTER_FEATURE.get() else {
        return false;
    };
    let (Some(id), Ok(label), Ok(category)) = (
        scoped_c(id),
        CString::new(label),
        CString::new(category),
    ) else {
        return false;
    };
    // SAFETY: every argument is a live NUL-terminated string for the duration of the call.
    unsafe { register(id.as_ptr(), label.as_ptr(), category.as_ptr(), u32::from(default)) > 0 }
}

/// Registers a numeric setting the user can slide, between `range` in `step` increments.
pub fn slider(
    id: &str,
    label: &str,
    category: &str,
    default: f32,
    range: RangeInclusive<f32>,
    step: f32,
) -> bool {
    let Some(register) = REGISTER_SLIDER.get() else {
        return false;
    };
    let (Some(id), Ok(label), Ok(category)) = (
        scoped_c(id),
        CString::new(label),
        CString::new(category),
    ) else {
        return false;
    };
    // SAFETY: every argument is a live NUL-terminated string for the duration of the call.
    unsafe {
        register(
            id.as_ptr(),
            label.as_ptr(),
            category.as_ptr(),
            f64::from(default),
            f64::from(*range.start()),
            f64::from(*range.end()),
            f64::from(step),
        ) > 0
    }
}

/// Registers a dropdown of `options`, starting on `default`.
pub fn choice(id: &str, label: &str, category: &str, options: &[&str], default: usize) -> bool {
    let Some(register) = REGISTER_CHOICE.get() else {
        return false;
    };
    let (Some(id), Ok(label), Ok(category), Ok(options)) = (
        scoped_c(id),
        CString::new(label),
        CString::new(category),
        CString::new(options.join("\n")),
    ) else {
        return false;
    };
    // SAFETY: every argument is a live NUL-terminated string for the duration of the call.
    unsafe {
        register(
            id.as_ptr(),
            label.as_ptr(),
            category.as_ptr(),
            options.as_ptr(),
            default as u32,
        ) > 0
    }
}

/// Registers a rebindable key. The loader watches `default_key` and reports presses and
/// releases through [`Mod::keybind`].
pub fn keybind(id: &str, label: &str, category: &str, default_key: &str) -> bool {
    let Some(register) = REGISTER_KEYBIND.get() else {
        return false;
    };
    let (Some(id), Ok(label), Ok(category), Ok(key)) = (
        scoped_c(id),
        CString::new(label),
        CString::new(category),
        CString::new(default_key),
    ) else {
        return false;
    };
    // SAFETY: every argument is a live NUL-terminated string for the duration of the call.
    unsafe { register(id.as_ptr(), label.as_ptr(), category.as_ptr(), key.as_ptr()) > 0 }
}

/// Registers a one-shot button; pressing it calls [`Mod::pressed`].
pub fn button(id: &str, label: &str, category: &str) -> bool {
    let Some(register) = REGISTER_BUTTON.get() else {
        return false;
    };
    let (Some(id), Ok(label), Ok(category)) = (
        scoped_c(id),
        CString::new(label),
        CString::new(category),
    ) else {
        return false;
    };
    // SAFETY: every argument is a live NUL-terminated string for the duration of the call.
    unsafe { register(id.as_ptr(), label.as_ptr(), category.as_ptr()) > 0 }
}

/// Whether the toggle is on. Unknown ids read as off.
#[must_use]
pub fn enabled(id: &str) -> bool {
    read_enabled(id) > 0
}

/// Turns a toggle on or off, as the user would.
pub fn set_enabled(id: &str, on: bool) {
    let Some(write) = WRITE_ENABLED.get() else {
        return;
    };
    let Some(id) = scoped_c(id) else {
        return;
    };
    // SAFETY: `write` is a loader-owned function pointer and `id` is NUL-terminated.
    unsafe { write(id.as_ptr(), u32::from(on)) };
}

/// A numeric setting's value, or 0.0 when unset.
#[must_use]
pub fn number(id: &str) -> f32 {
    let Some(read) = READ_NUMBER.get() else {
        return 0.0;
    };
    let Some(id) = scoped_c(id) else {
        return 0.0;
    };
    // SAFETY: `read` is a loader-owned function pointer and `id` is NUL-terminated.
    unsafe { read(id.as_ptr(), f64::NAN) as f32 }
}

/// Sets a numeric setting, as the user would.
pub fn set_number(id: &str, value: f32) {
    let Some(write) = WRITE_NUMBER.get() else {
        return;
    };
    let Some(id) = scoped_c(id) else {
        return;
    };
    // SAFETY: `write` is a loader-owned function pointer and `id` is NUL-terminated.
    unsafe { write(id.as_ptr(), f64::from(value)) };
}

/// A dropdown's selected index, or 0 when unset.
#[must_use]
pub fn choice_index(id: &str) -> usize {
    let Some(read) = READ_CHOICE.get() else {
        return 0;
    };
    let Some(id) = scoped_c(id) else {
        return 0;
    };
    // SAFETY: `read` is a loader-owned function pointer and `id` is NUL-terminated.
    usize::try_from(unsafe { read(id.as_ptr()) }).unwrap_or(0)
}

/// A keybind's key name, empty when unset.
#[must_use]
pub fn key(id: &str) -> String {
    let Some(read) = READ_KEY_NAME.get() else {
        return String::new();
    };
    let Some(id) = scoped_c(id) else {
        return String::new();
    };
    let mut buffer = [0 as c_char; 48];
    // SAFETY: `read` writes at most `buffer.len()` bytes, NUL-terminated.
    if unsafe { read(id.as_ptr(), buffer.as_mut_ptr(), buffer.len()) } <= 0 {
        return String::new();
    }
    // SAFETY: the loader wrote a NUL-terminated key name.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// A mod-private value saved next to the mods, or `None` when unset.
#[must_use]
pub fn config(key: &str) -> Option<String> {
    let read = READ_CONFIG.get()?;
    let key = CString::new(key).ok()?;
    let mut buffer = [0 as c_char; 512];
    // SAFETY: `read` writes at most `buffer.len()` bytes, NUL-terminated.
    if unsafe { read(key.as_ptr(), buffer.as_mut_ptr(), buffer.len()) } <= 0 {
        return None;
    }
    // SAFETY: the loader wrote a NUL-terminated value.
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// The same as [`config`], under a name that cannot collide with the [`config`] module when
/// both are in scope.
#[must_use]
pub fn config_value(key: &str) -> Option<String> {
    config(key)
}

/// Saves a mod-private value next to the mods.
pub fn set_config(key: &str, value: &str) {
    let Some(write) = WRITE_CONFIG.get() else {
        return;
    };
    let (Ok(key), Ok(value)) = (CString::new(key), CString::new(value)) else {
        return;
    };
    // SAFETY: both arguments are live NUL-terminated strings for the duration of the call.
    unsafe { write(key.as_ptr(), value.as_ptr()) };
}

fn read_enabled(id: &str) -> i32 {
    let Some(read) = READ_ENABLED.get() else {
        return -1;
    };
    let Some(id) = scoped_c(id) else {
        return -1;
    };
    // SAFETY: `read` is a loader-owned function pointer and `id` is NUL-terminated.
    unsafe { read(id.as_ptr()) }
}

/// Prefixes `id` with this mod's name, the scope the loader registers it under, as a
/// NUL-terminated C string.
fn scoped_c(id: &str) -> Option<CString> {
    CString::new(scoped(id)?).ok()
}

/// Prefixes `id` with this mod's name.
fn scoped(id: &str) -> Option<String> {
    let name = MOD_NAME.get()?;
    Some(format!("{name}:{id}"))
}

/// The next queued event, as `(kind, id, value)`. [`export_mod!`] drains these into the
/// matching [`Mod`] method before each [`Mod::tick`].
#[must_use]
pub fn take_event() -> Option<(String, String, String)> {
    let take = TAKE_EVENT.get()?;
    let mut buffer = [0 as c_char; 512];
    // SAFETY: `take` writes at most `buffer.len()` bytes, NUL-terminated.
    if unsafe { take(buffer.as_mut_ptr(), buffer.len()) } <= 0 {
        return None;
    }
    // SAFETY: the loader wrote a NUL-terminated event line.
    let line = unsafe { CStr::from_ptr(buffer.as_ptr()) }.to_str().ok()?;
    let mut parts = line.split('\t');
    let kind = parts.next()?.to_owned();
    let id = parts.next()?.to_owned();
    let value = parts.next().unwrap_or("").to_owned();
    let id = id.split_once(':').map_or(id.clone(), |(_, id)| id.to_owned());
    Some((kind, id, value))
}

fn message_level(level: u32) -> &'static str {
    match level {
        level::ERROR => "error",
        level::WARN => "warn",
        _ => "info",
    }
}

/// Stores the host table for this library instance. Called by the generated
/// `cinnaloader_init`; a mod should not call it directly.
///
/// # Safety
/// `api` must be null or a valid, host-owned [`HostApi`] whose pointers stay live.
#[doc(hidden)]
pub unsafe fn install(api: *const HostApi) -> i32 {
    if api.is_null() {
        return -1;
    }
    // SAFETY: the caller guarantees a valid pointer.
    let api = unsafe { &*api };
    if api.abi_version != ABI_VERSION {
        return -2;
    }
    if let Some(log) = api.log {
        let _ = LOG.set(log);
    }
    if let Some(overlay_line) = api.overlay_line {
        let _ = OVERLAY_LINE.set(overlay_line);
    }
    if let Some(toggle_gui) = api.toggle_gui {
        let _ = TOGGLE_GUI.set(toggle_gui);
    }
    // `mods_dir` and `mod_name` are required: ids cannot be scoped without the name.
    if !api.mods_dir.is_null() {
        // SAFETY: the host keeps `mods_dir` valid for the process.
        if let Ok(dir) = unsafe { CStr::from_ptr(api.mods_dir) }.to_str() {
            let _ = MODS_DIR_PATH.set(dir.to_owned());
        }
    }
    if api.mod_name.is_null() {
        return -4;
    }
    // SAFETY: the host keeps `mod_name` valid for the process.
    if let Ok(name) = unsafe { CStr::from_ptr(api.mod_name) }.to_str() {
        let _ = MOD_NAME.set(name.to_owned());
    }
    store(&REGISTER_SLIDER, api.slider);
    store(&REGISTER_CHOICE, api.choice);
    store(&REGISTER_KEYBIND, api.keybind);
    store(&REGISTER_BUTTON, api.button);
    store(&READ_ENABLED, api.enabled);
    store(&WRITE_ENABLED, api.set_enabled);
    store(&READ_NUMBER, api.number);
    store(&WRITE_NUMBER, api.set_number);
    store(&READ_CHOICE, api.choice_index);
    store(&READ_KEY_NAME, api.key_name);
    store(&TAKE_EVENT, api.take_event);
    store(&READ_CONFIG, api.config);
    store(&WRITE_CONFIG, api.set_config);
    store(&DECLARE, api.declare);
    store(&GRAPH, api.graph);
    store(&MODS_COMMAND, api.mods_command);
    store(&REPORT, api.report);
    store(&REGISTER_FEATURE, api.feature);
    0
}

/// Fills a host-service slot once.
fn store<T>(slot: &OnceLock<T>, value: Option<T>) {
    if let Some(value) = value {
        let _ = slot.set(value);
    }
}

/// What the loader drives. Every method has a default, so a mod implements only what it
/// needs.
///
/// The phases the loader walks, and what is safe in each, are in [`lifecycle`].
pub trait Mod: Default + Send + 'static {
    /// Runs before [`Self::load`]. Returning false refuses this loader, and the mod is not
    /// loaded at all — the place to gate on a loader version that is too old.
    fn compatibility(&mut self, _loader: &modinfo::ModVersion) -> bool {
        true
    }

    /// Runs once, after the host table is installed. Declare the manifest and register
    /// settings here.
    fn load(&mut self) {}

    /// Runs at the end of `load` when the mod's version changed since its last run, so it
    /// can move its own saved data. [`previous_version`] is the version that last ran.
    fn migrate(&mut self, _from: &modinfo::ModVersion, _to: &modinfo::ModVersion) {}

    /// Runs just before the first tick, once the loader has applied the mod's saved
    /// settings.
    fn config_loaded(&mut self) {}

    /// Runs after every mod has loaded.
    fn initialize(&mut self) {}

    /// Runs immediately after [`Self::initialize`].
    fn post_initialize(&mut self) {}

    /// Runs once every mod is loaded and initialised.
    fn ready(&mut self) {}

    /// Runs before [`Self::tick`] each tick.
    fn tick_start(&mut self, _dt_millis: u32) {}

    /// Runs after [`Self::tick`] each tick.
    fn tick_end(&mut self, _dt_millis: u32) {}

    /// Runs as the client starts a frame. In-client only; nothing blocking belongs here.
    fn render_start(&mut self) {}

    /// Runs as the client finishes a frame.
    fn render_end(&mut self) {}

    /// Runs when the loader catches a panic from this mod. The mod is switched off after.
    fn error(&mut self, _kind: &str, _message: &str) {}

    /// Runs after [`Self::unload`], as the process goes away.
    fn shutdown(&mut self) {}
    /// Runs on every loader tick with the milliseconds since the previous one.
    fn tick(&mut self, _dt_millis: u32) {}
    /// Runs as the loader unloads the mod.
    fn unload(&mut self) {}
    /// Runs when the user flips one of this mod's toggles.
    fn toggle(&mut self, _id: &str, _on: bool) {}
    /// Runs when the user drags one of this mod's sliders.
    fn slider(&mut self, _id: &str, _value: f32) {}
    /// Runs when the user picks one of this mod's dropdown options.
    fn choice(&mut self, _id: &str, _index: usize) {}
    /// Runs when one of this mod's keybinds is pressed or released.
    fn keybind(&mut self, _id: &str, _down: bool) {}
    /// Runs when the user presses one of this mod's buttons.
    fn pressed(&mut self, _id: &str) {}
}

/// What the host provides, read from the loader's report once.
static HOST_FACTS: OnceLock<modinfo::HostFacts> = OnceLock::new();

/// The loader, game and process facts. Read once; they do not change while a mod runs.
#[must_use]
pub fn host_facts() -> modinfo::HostFacts {
    HOST_FACTS
        .get_or_init(|| {
            let text = read_report().unwrap_or_default();
            let mut facts = modinfo::HostFacts::decode(&text);
            if facts.api_version == 0 {
                facts.api_version = ABI_VERSION;
            }
            facts
        })
        .clone()
}

/// The ABI version the loader provides, or the one this SDK was built for.
#[must_use]
pub fn hosted_api_version() -> u32 {
    let api = host_facts().api_version;
    if api == 0 { ABI_VERSION } else { api }
}

/// The loader's version, for [`Mod::compatibility`].
#[must_use]
pub fn loader_version() -> modinfo::ModVersion {
    modinfo::ModVersion::parse(&host_facts().loader_version)
}

/// The migration a declaration asked for, waiting for the end of `load`.
static PENDING_MIGRATION: OnceLock<(modinfo::ModVersion, modinfo::ModVersion)> = OnceLock::new();

/// Records a declaration for the loader and queues any migration it implies. Called by
/// [`metadata::declare`].
#[doc(hidden)]
pub fn note_declaration(manifest: &modinfo::ModManifest) {
    if let Some(previous) = lifecycle::previous_version() {
        if previous != manifest.version {
            let _ = PENDING_MIGRATION.set((previous, manifest.version.clone()));
        }
    }
}

/// Turns a caught panic into a line worth logging.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "panicked".to_owned()
    }
}

/// Runs one phase for a mod, catching a panic so one bad mod does not take the loader with
/// it. Returns non-zero when the mod panicked, which is the loader's cue to switch it off.
fn guarded<T: Mod>(
    module: &mut T,
    kind: &str,
    body: impl FnOnce(&mut T) -> bool,
) -> i32 {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(module)));
    match caught {
        Ok(true) => 0,
        Ok(false) => {
            log(level::WARN, &format!("{kind} did not run"));
            0
        }
        Err(payload) => {
            let message = panic_message(payload);
            log(level::ERROR, &format!("{kind} panicked: {message}"));
            diagnostics::note_crash(kind, &message);
            lifecycle::enter(lifecycle::Phase::Error);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                module.error("panic", &message);
            }));
            1
        }
    }
}

/// The body of a mod's `init`. Called by [`export_mod!`].
#[doc(hidden)]
pub fn load_body<T: Mod>(module: &mut T) -> i32 {
    tasks::record_main_thread();
    let loader = loader_version();
    let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        !module.compatibility(&loader)
    }))
    .unwrap_or(false);
    if refused {
        log(level::ERROR, "refused: this mod does not accept this loader");
        return 2;
    }
    let code = guarded(module, "load", |module| {
        lifecycle::enter(lifecycle::Phase::Load);
        module.load();
        true
    });
    if code != 0 {
        return code;
    }
    if let Some((from, to)) = PENDING_MIGRATION.get() {
        log(
            level::INFO,
            &format!("migrating saved settings from {from} to {to}"),
        );
        let code = guarded(module, "migrate", |module| {
            module.migrate(from, to);
            true
        });
        if code != 0 {
            return code;
        }
    }
    lifecycle::record_version(&metadata::manifest().version);
    // Fired after `load` rather than before it, so a mod that subscribes while loading
    // still hears that the game started.
    events::publish(events::Event::new(events::kind::GAME_STARTED).with_text(&metadata::id()));
    events::dispatch_pending();
    0
}

/// The body of a mod's `frame`. Called by [`export_mod!`].
#[doc(hidden)]
pub fn frame_body<T: Mod>(module: &mut T, dt_millis: u32, first: bool) -> i32 {
    if first {
        // The loader applies saved settings between `load` and this first tick, so this is
        // the first moment they are in effect.
        let code = guarded(module, "config_loaded", |module| {
            module.config_loaded();
            true
        });
        if code != 0 {
            return code;
        }
    }
    guarded(module, "tick", |module| {
        while let Some((kind, id, value)) = take_event() {
            match kind.as_str() {
                "toggle" => {
                    events::publish(
                        events::Event::new(events::kind::SETTINGS_CHANGED)
                            .with_text(&format!("{id}={value}")),
                    );
                    module.toggle(&id, value == "1");
                }
                "number" => {
                    events::publish(
                        events::Event::new(events::kind::SETTINGS_CHANGED)
                            .with_value(value.parse().unwrap_or_default())
                            .with_text(&id),
                    );
                    module.slider(&id, value.parse().unwrap_or_default());
                }
                "choice" => {
                    events::publish(
                        events::Event::new(events::kind::SETTINGS_CHANGED)
                            .with_count(value.parse().unwrap_or_default())
                            .with_text(&id),
                    );
                    module.choice(&id, value.parse().unwrap_or_default());
                }
                "keybind" => {
                    events::publish(
                        events::Event::cancellable(events::kind::INPUT)
                            .with_flag(value == "1")
                            .with_text(&id),
                    );
                    module.keybind(&id, value == "1");
                }
                "press" => {
                    events::publish(
                        events::Event::cancellable(events::kind::INPUT)
                            .with_flag(true)
                            .with_text(&id),
                    );
                    module.pressed(&id);
                }
                "enabled" | "disabled" => {
                    let on = kind == "enabled";
                    events::publish(
                        events::Event::new(if on {
                            events::kind::MOD_ENABLED
                        } else {
                            events::kind::MOD_DISABLED
                        })
                        .with_flag(on)
                        .with_text(&id)
                        .from(&id),
                    );
                }
                _ => {}
            }
        }
        lifecycle::enter(lifecycle::Phase::TickStart);
        module.tick_start(dt_millis);
        lifecycle::enter(lifecycle::Phase::Tick);
        module.tick(dt_millis);
        lifecycle::enter(lifecycle::Phase::TickEnd);
        module.tick_end(dt_millis);
        // One frame is one tick for this host, which is what `game` counts.
        game::note_frame(dt_millis);
        // Whatever was scheduled for this tick runs now, before the frame ends.
        tasks::run_due(dt_millis);
        true
    })
}

/// The body of a mod's `phase`. Called by [`export_mod!`].
#[doc(hidden)]
pub fn phase_body<T: Mod>(module: &mut T, code: u32) -> i32 {
    let Some(phase) = lifecycle::Phase::from_code(code) else {
        return 0;
    };
    guarded(module, phase.name(), |module| {
        lifecycle::enter(phase);
        match phase {
            lifecycle::Phase::Initialize => module.initialize(),
            lifecycle::Phase::PostInitialize => module.post_initialize(),
            lifecycle::Phase::Ready => module.ready(),
            lifecycle::Phase::RenderStart => module.render_start(),
            lifecycle::Phase::RenderEnd => module.render_end(),
            lifecycle::Phase::Shutdown => module.shutdown(),
            lifecycle::Phase::Error => return false,
            _ => return false,
        }
        true
    })
}

/// The body of a mod's `shutdown`. Called by [`export_mod!`].
#[doc(hidden)]
pub fn unload_body<T: Mod>(module: &mut T) -> i32 {
    let code = guarded(module, "unload", |module| {
        lifecycle::enter(lifecycle::Phase::Unload);
        module.unload();
        true
    });
    if code != 0 {
        return code;
    }
    lifecycle::run_cleanups();
    let code = guarded(module, "shutdown", |module| {
        lifecycle::enter(lifecycle::Phase::Shutdown);
        module.shutdown();
        true
    });
    lifecycle::run_shutdown_handlers();
    code
}

/// Generates the C entry points for a [`Mod`] type.
///
/// ```
/// # use cinnaloader::{export_mod, Mod};
/// #[derive(Default)]
/// struct Example;
/// impl Mod for Example {}
/// export_mod!(Example);
/// ```
#[macro_export]
macro_rules! export_mod {
    ($ty:ty) => {
        $crate::export_mod!($ty, manifest = $crate::modinfo::ModManifest::new(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION"),
        ));
    };
    ($ty:ty, manifest = $manifest:expr) => {
        /// The manifest the loader reads *before* this mod is loaded, so it can resolve
        /// dependencies and decide the load order. This is the same text the mod's
        /// `manifest()` reports.
        /// # Safety
        /// `out` is null or points to `capacity` writable bytes.
        #[no_mangle]
        pub unsafe extern "C" fn cinnaloader_manifest(out: *mut i8, capacity: usize) -> i32 {
            static TEXT: ::std::sync::OnceLock<::std::ffi::CString> =
                ::std::sync::OnceLock::new();
            let text = TEXT.get_or_init(|| {
                let manifest = $manifest;
                ::std::ffi::CString::new(manifest.encode()).unwrap_or_default()
            });
            let bytes = text.as_bytes();
            if out.is_null() || bytes.len() + 1 > capacity {
                return 0;
            }
            // SAFETY: the caller offered `capacity` writable bytes and the text fits.
            unsafe {
                ::std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    out as *mut u8,
                    bytes.len(),
                );
                *out.add(bytes.len()) = 0;
            }
            1
        }

        /// The declared manifest, so `load` can read what the loader already has.
        #[must_use]
        #[doc(hidden)]
        pub fn declared_manifest() -> $crate::modinfo::ModManifest {
            $manifest
        }
        static CINNALOADER_MOD: ::std::sync::OnceLock<::std::sync::Mutex<$ty>> =
            ::std::sync::OnceLock::new();

        #[no_mangle]
        pub extern "C" fn cinnaloader_abi_version() -> u32 {
            $crate::ABI_VERSION
        }

        /// # Safety
        /// `api` is null or a valid host table pointer.
        #[no_mangle]
        pub unsafe extern "C" fn cinnaloader_init(api: *const $crate::HostApi) -> i32 {
            let code = unsafe { $crate::install(api) };
            if code != 0 {
                return code;
            }
            $crate::metadata::adopt($manifest);
            let mut module = <$ty as ::std::default::Default>::default();
            let code = $crate::load_body(&mut module);
            if code != 0 {
                return code;
            }
            if CINNALOADER_MOD
                .set(::std::sync::Mutex::new(module))
                .is_err()
            {
                return -3;
            }
            0
        }

        static CINNALOADER_FIRST_TICK: ::std::sync::atomic::AtomicBool =
            ::std::sync::atomic::AtomicBool::new(true);

        /// One loader tick. Returns non-zero when the mod panicked and should be switched
        /// off, so one bad mod cannot take the process with it.
        #[no_mangle]
        pub extern "C" fn cinnaloader_frame(dt_millis: u32) -> i32 {
            let Some(module) = CINNALOADER_MOD.get() else {
                return 0;
            };
            let Ok(mut module) = module.lock() else {
                return 1;
            };
            let first = CINNALOADER_FIRST_TICK.swap(
                false,
                ::std::sync::atomic::Ordering::SeqCst,
            );
            $crate::frame_body(&mut *module, dt_millis, first)
        }

        /// One lifecycle phase, as the loader walks them. See `cinnaloader::lifecycle`.
        #[no_mangle]
        pub extern "C" fn cinnaloader_phase(code: u32) -> i32 {
            let Some(module) = CINNALOADER_MOD.get() else {
                return 0;
            };
            let Ok(mut module) = module.lock() else {
                return 1;
            };
            $crate::phase_body(&mut *module, code)
        }

        #[no_mangle]
        pub extern "C" fn cinnaloader_shutdown() -> i32 {
            let Some(module) = CINNALOADER_MOD.get() else {
                return 0;
            };
            let Ok(mut module) = module.lock() else {
                return 1;
            };
            $crate::unload_body(&mut *module)
        }
    };
}

/// Everything a mod needs: `use cinnaloader::prelude::*;`
pub mod prelude {
    pub use crate::metadata::{
        authors, build_number, build_timestamp, cache_directory, category,
        declared_capabilities,
        config_directory, context, contributors, credits, data_directory, declare, declared,
        description,
        directory, entrypoint, environment, git_commit, icon, id, instance, is_development,
        is_enabled, is_reloading, license, loader_version, manifest, manifest_version, metadata,
        name, namespace, permissions, plugin_directory, reported, resource_directory, source_url,
        state,
        tags, version, website,
    };
    pub use crate::capability::{self as capabilities, Capability};
    pub use crate::compat;
    pub use crate::blocks::{
        self as blocks, BlockBehaviour, BlockDescription, BlockMaterial, BlockProperty,
        BlockShapes, ToolRequirement,
    };
    pub use crate::components::{
        self as components, Attribute, AttributeDefinition, AttributeModifier, AttributeSet,
        ComponentData, ComponentField, ComponentSet, ComponentType, ComponentValue, ModifierKind,
    };
    pub use crate::config::{
        self as config, ConfigCategory, ConfigComment, ConfigConstraint, ConfigEntry, ConfigGroup,
        ConfigMigration, ConfigRestartRequirement, ConfigSchema, ConfigType, ConfigValidator,
        ConfigValue, ConfigVisibility, Transaction,
    };
    pub use crate::deps::{
        CircularDependencyReport, DependencyGraph, DependencyKind, DependencyResolver,
        LoadOrderConstraint, ModConflict, ModDependency, ModResolutionError, OptionalDependency,
        ResolutionErrorKind, SoftDependency, VersionedDependency,
    };
    pub use crate::effects::{
        self as effects, damage, DamageInstance, DamageModifier, DamageModifierKind,
        DamageSource, DamageType, EffectCategory, EffectType,
    };
    pub use crate::entities::{
        self as entities, EntityComponent, EntitySize, EntityType,
    };
    pub use crate::events::{
        self as events, DispatchPhase, Event, ListenerInfo, Payload, Priority, kind,
    };
    pub use crate::fs::{self as fs, File, FsError, Metadata as FileMetadata, Watch};
    pub use crate::game::{self as game, Request, RequestOutcome, SessionInfo, Window};
    pub use crate::i18n::{self as i18n, Language, NumberStyle};
    pub use crate::inventory::{
        self as inventory, ContainerType, Inventory, InventoryError, SlotKind,
        HOTBAR_FIRST, HOTBAR_LAST, MAIN_FIRST, MAIN_LAST, OFFHAND, ARMOUR_FIRST, ARMOUR_LAST,
        PLAYER_SLOTS,
    };
    pub use crate::items::{self as items, Component as ItemComponent, ItemStack, ItemType};
    pub use crate::diagnostics;
    pub use crate::log::{self as log, Level, Logger, Record};
    pub use crate::mods;
    pub use crate::physics::{
        self as physics, bounding_box, Body, Ray, RayHit, GRAVITY, TERMINAL_VELOCITY,
    };
    pub use crate::player::{
        self as player, Effect, GameMode, PlayerPermission, PlayerState, Statistics,
    };
    pub use crate::permissions::{
        self as permissions, AuditEntry, AuditKind, Grant, TrustLevel,
    };
    pub use crate::raycast::{
        self as raycast, from_camera, from_entity as raycast_from_entity, from_position,
        from_body, EntityBox, RaycastQuery, RaycastResult, Target,
        CAMERA_REACH, REACH_CREATIVE, REACH_SURVIVAL,
    };
    pub use crate::resources::{
        self as resources, Provider, ResourceDependency, ResourceId, ResourceKind,
        ResourceLocation, ResourceManager, ResourceMetadata, ResourceOverride, ResourcePack,
        ResourceReloadListener, ResourceStack,
    };
    pub use crate::world::{
        self as world, dimensions, AmbientProperties, DimensionDescription, DimensionKind,
        Refusal, RespawnRules, WorldInfo,
    };
    pub use crate::storage::{
        self as storage, Format, Namespace, Storage, StoreError, StoreTransaction,
    };
    pub use crate::tasks::{
        self as tasks, AsyncResult, CancellationToken, JobGroup, JobQueue, MainThreadExecutor,
        NetworkExecutor, RenderThreadExecutor, TaskExecutor, TaskHandle, TaskMetrics, TaskPriority,
        TaskResult, TaskStatus, ThreadAffinity, WorkerPool,
    };
    pub use crate::lifecycle::{
        cancel_pending_hooks, current_phase, defer_until_ready, is_initialized, is_ready,
        previous_version, register_cleanup, register_shutdown_handler, Phase,
    };
    pub use crate::modinfo::{
        BuildInfo, Environment, ModContext, ModDirectory, ModInfo, ModInstance,
        ModManifest, ModMetadata, ModNamespace, ModVersion, Permission, State,
    };
    pub use crate::version::{
        ApiStability, CompatibilityReport, CompatibilityRule, DeprecationInfo, FeatureAvailability,
        FeatureFlag, Version, VersionConstraint, VersionMapping, VersionPredicate, VersionRange,
        VersionReq,
    };
    pub use crate::{
        button, choice, choice_index, config_value, enabled, error, export_mod, feature,
        hosted_api_version, info, key,
        keybind, meta, mods_dir, number, overlay_line, set_config, set_enabled, set_number,
        slider, toggle_gui, warn, Mod, Timer, OFF, ON,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    /// The host slots are process-wide, so tests that install a table run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL.lock().unwrap_or_else(|error| error.into_inner())
    }

    static REGISTERED: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());
    static ENABLED: AtomicBool = AtomicBool::new(true);
    static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    unsafe extern "C" fn probe_feature(
        id: *const c_char,
        label: *const c_char,
        category: *const c_char,
        _default_on: u32,
    ) -> i32 {
        let text = |ptr: *const c_char| {
            unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .unwrap_or_default()
                .to_owned()
        };
        REGISTERED
            .lock()
            .unwrap()
            .push((text(id), text(label), text(category)));
        1
    }

    unsafe extern "C" fn probe_enabled(_id: *const c_char) -> i32 {
        i32::from(ENABLED.load(Ordering::SeqCst))
    }

    unsafe extern "C" fn probe_take_event(out: *mut c_char, capacity: usize) -> i32 {
        let Some(line) = EVENTS.lock().unwrap().pop() else {
            return 0;
        };
        let bytes = line.as_bytes();
        if bytes.len() + 1 > capacity {
            return 0;
        }
        // SAFETY: the caller offered `capacity` writable bytes and the line fits.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), out as *mut u8, bytes.len());
            *out.add(bytes.len()) = 0;
        }
        1
    }

    fn probe(name: &str) -> HostApi {
        let name = CString::new(name).unwrap();
        let name = Box::leak(Box::new(name));
        HostApi {
            abi_version: ABI_VERSION,
            log: None,
            mods_dir: std::ptr::null(),
            overlay_line: None,
            toggle_gui: None,
            mod_name: name.as_ptr(),
            feature: Some(probe_feature),
            slider: None,
            choice: None,
            keybind: None,
            button: None,
            enabled: Some(probe_enabled),
            set_enabled: None,
            number: None,
            set_number: None,
            choice_index: None,
            key_name: None,
            take_event: Some(probe_take_event),
            config: None,
            set_config: None,
            declare: None,
            report: None,
            graph: None,
            mods_command: None,
        }
    }

    #[test]
    fn abi_version_is_stable() {
        assert_eq!(ABI_VERSION, 8);
    }

    #[test]
    fn a_null_table_is_refused() {
        assert_eq!(unsafe { install(std::ptr::null()) }, -1);
    }

    #[test]
    fn a_mismatched_version_is_refused() {
        let _serial = serial();
        let mut api = probe("probe");
        api.abi_version = ABI_VERSION + 1;
        assert_eq!(unsafe { install(&api) }, -2);
    }

    #[test]
    fn a_table_without_a_mod_name_is_refused() {
        let _serial = serial();
        let mut api = probe("probe");
        api.mod_name = std::ptr::null();
        assert_eq!(unsafe { install(&api) }, -4);
    }

    #[test]
    fn a_feature_is_registered_under_the_mods_own_name() {
        let _serial = serial();
        REGISTERED.lock().unwrap().clear();
        assert_eq!(unsafe { install(&probe("probe")) }, 0);
        assert!(feature("sprint", "Sprint", "Movement", ON));
        let registered = REGISTERED.lock().unwrap();
        let (id, label, category) = registered.last().unwrap();
        assert_eq!(id, "probe:sprint");
        assert_eq!(label, "Sprint");
        assert_eq!(category, "Movement");
    }

    #[test]
    fn enabled_reads_the_host_state() {
        let _serial = serial();
        assert_eq!(unsafe { install(&probe("probe")) }, 0);
        ENABLED.store(true, Ordering::SeqCst);
        assert!(enabled("sprint"));
        ENABLED.store(false, Ordering::SeqCst);
        assert!(!enabled("sprint"));
    }

    #[test]
    fn an_event_arrives_unscoped_and_split() {
        let _serial = serial();
        assert_eq!(unsafe { install(&probe("probe")) }, 0);
        ENABLED.store(true, Ordering::SeqCst);
        EVENTS
            .lock()
            .unwrap()
            .push("toggle\tprobe:sprint\t1".to_owned());
        assert_eq!(
            take_event(),
            Some(("toggle".to_owned(), "sprint".to_owned(), "1".to_owned()))
        );
        assert_eq!(take_event(), None);
    }

    #[derive(Default)]
    struct Recorder {
        seen: Vec<&'static str>,
        errors: Vec<String>,
        boom: bool,
    }

    impl Mod for Recorder {
        fn load(&mut self) {
            self.seen.push("load");
        }
        fn initialize(&mut self) {
            self.seen.push("initialize");
        }
        fn post_initialize(&mut self) {
            self.seen.push("post_initialize");
        }
        fn ready(&mut self) {
            self.seen.push("ready");
        }
        fn config_loaded(&mut self) {
            self.seen.push("config_loaded");
        }
        fn tick_start(&mut self, _dt: u32) {
            self.seen.push("tick_start");
        }
        fn tick(&mut self, _dt: u32) {
            self.seen.push("tick");
            if self.boom {
                panic!("boom");
            }
        }
        fn tick_end(&mut self, _dt: u32) {
            self.seen.push("tick_end");
        }
        fn error(&mut self, kind: &str, message: &str) {
            self.errors.push(format!("{kind}: {message}"));
        }
        fn unload(&mut self) {
            self.seen.push("unload");
        }
        fn shutdown(&mut self) {
            self.seen.push("shutdown");
        }
    }

    #[test]
    fn the_phases_reach_the_mod_in_order() {
        let mut module = Recorder::default();
        assert_eq!(load_body(&mut module), 0);
        assert_eq!(phase_body(&mut module, crate::lifecycle::Phase::Initialize.code()), 0);
        assert_eq!(
            phase_body(&mut module, crate::lifecycle::Phase::PostInitialize.code()),
            0
        );
        assert_eq!(phase_body(&mut module, crate::lifecycle::Phase::Ready.code()), 0);
        assert_eq!(frame_body(&mut module, 16, true), 0, "first tick");
        assert_eq!(frame_body(&mut module, 16, false), 0, "later tick");
        assert_eq!(unload_body(&mut module), 0);
        assert_eq!(
            module.seen,
            vec![
                "load",
                "initialize",
                "post_initialize",
                "ready",
                "config_loaded",
                "tick_start",
                "tick",
                "tick_end",
                "tick_start",
                "tick",
                "tick_end",
                "unload",
                "shutdown",
            ]
        );
    }

    #[test]
    fn a_panicking_mod_is_reported_and_isolated() {
        let mut module = Recorder {
            boom: true,
            ..Recorder::default()
        };
        assert_eq!(frame_body(&mut module, 16, true), 1, "the loader is told");
        assert_eq!(module.errors, vec!["panic: boom".to_owned()]);
        assert_eq!(current_error_phase(), crate::lifecycle::Phase::Error);
    }

    fn current_error_phase() -> crate::lifecycle::Phase {
        crate::lifecycle::current_phase()
    }

    #[test]
    fn a_rate_fires_on_its_interval() {
        let mut every = Timer::every(100);
        assert!(!every.ready(60));
        assert!(every.ready(60), "100 ms elapsed");
        assert!(!every.ready(60));
        assert!(every.ready(60));
        let mut once = Timer::after(50);
        assert!(!once.ready(10));
        assert!(once.ready(50));
        assert!(!once.ready(500), "a one-shot does not repeat");
        let mut unset = Timer::default();
        assert!(!unset.ready(10_000));
    }
}
