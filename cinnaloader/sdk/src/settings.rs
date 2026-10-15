//! Settings: the typed options a mod owns, and the client's own options this host cannot
//! reach.
//!
//! An **option** is a value a mod ships: an id, a kind, a default, optional bounds or a
//! choice list, a label, a tooltip and the category a panel groups it under. Registering one
//! is real and a second with the same id is refused; [`set`] checks the kind, the bounds and
//! the choice list and answers a real error when any is wrong; a change fires the handlers
//! and the §6 bus with the old and the new value and fires nothing when the value did not
//! move; [`reset`] and [`reset_all`] put options back to their defaults, and a [`Profile`]
//! applies a named set through [`apply`] and reports what it changed. A whole set renders to
//! and parses from one [`ConfigValue`], and [`save`]/[`load`] keep it in a persistence store.
//!
//! The client's own video, audio, controls and accessibility options are the part this host
//! cannot reach: reading one, or changing one, is the running client's state, and needs a
//! hook this host does not have. Every such getter answers `None` with the reason from
//! [`why_unknown`], and [`set_client`] answers `false`.
//!
//! ```
//! use cinnaloader::config::{ConfigType, ConfigValue};
//! use cinnaloader::settings;
//!
//! settings::register(
//!     settings::Setting::new("hello:volume", ConfigType::Float, ConfigValue::Float(0.5))
//!         .label("Volume")
//!         .category("audio")
//!         .bounded(0.0, 1.0),
//! );
//! assert_eq!(settings::get("hello:volume"), Some(ConfigValue::Float(0.5)));
//! assert!(settings::set("hello:volume", ConfigValue::Float(0.8)).unwrap().is_some());
//! assert_eq!(settings::reset("hello:volume").unwrap().unwrap().new, ConfigValue::Float(0.5));
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config::{ConfigType, ConfigValue};
use crate::events::{self, Event};

/// The §6 kind fired when a setting's value changes.
pub const EVENT_CHANGED: &str = "settings_changed";

/// The persistence store [`save`] and [`load`] keep the set in.
pub const STORE: &str = "settings";

/// Why a value was refused, or a set could not be read.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingError {
    WrongKind {
        setting: String,
        wanted: String,
        found: String,
    },
    OutOfRange {
        setting: String,
        min: f64,
        max: f64,
        got: f64,
    },
    BadChoice {
        setting: String,
        choices: Vec<String>,
        got: String,
    },
    UnknownSetting {
        setting: String,
    },
    NotASet {
        found: String,
    },
}

impl fmt::Display for SettingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongKind { setting, wanted, found } => {
                write!(formatter, "{setting} wants {wanted}, got {found}")
            }
            Self::OutOfRange { setting, min, max, got } => {
                write!(formatter, "{setting} is {got}, outside {min}..{max}")
            }
            Self::BadChoice { setting, choices, got } => {
                write!(formatter, "{setting} is {got}, not one of {}", choices.join(", "))
            }
            Self::UnknownSetting { setting } => {
                write!(formatter, "{setting} is not a registered setting")
            }
            Self::NotASet { found } => {
                write!(formatter, "a settings set is an object, got {found}")
            }
        }
    }
}

/// One option: what it holds, what it may hold, and how it is shown.
#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub id: String,
    pub owner: String,
    pub kind: ConfigType,
    pub default: ConfigValue,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub choices: Vec<String>,
    pub label: String,
    pub tooltip: String,
    pub category: String,
}

impl Setting {
    #[must_use]
    pub fn new(id: &str, kind: ConfigType, default: ConfigValue) -> Self {
        Self {
            id: id.to_owned(),
            owner: crate::metadata::id(),
            kind,
            default,
            min: None,
            max: None,
            choices: Vec::new(),
            label: id.to_owned(),
            tooltip: String::new(),
            category: String::new(),
        }
    }

    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = label.to_owned();
        self
    }

    #[must_use]
    pub fn tooltip(mut self, tooltip: &str) -> Self {
        self.tooltip = tooltip.to_owned();
        self
    }

    #[must_use]
    pub fn category(mut self, category: &str) -> Self {
        self.category = category.to_owned();
        self
    }

    #[must_use]
    pub fn bounded(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    #[must_use]
    pub fn choices(mut self, choices: &[&str]) -> Self {
        self.choices = choices.iter().map(|choice| (*choice).to_owned()).collect();
        self
    }

    /// The choice list, when the option is an enum.
    #[must_use]
    pub fn choice_list(&self) -> Option<&[String]> {
        (self.kind == ConfigType::Enum).then_some(self.choices.as_slice())
    }

    /// Whether `value` may be stored here, and why not when it may not.
    pub fn validate(&self, value: &ConfigValue) -> Result<(), SettingError> {
        if !self.kind.accepts(value.kind()) {
            return Err(SettingError::WrongKind {
                setting: self.id.clone(),
                wanted: self.kind.as_str().to_owned(),
                found: value.kind().as_str().to_owned(),
            });
        }
        if let Some(choices) = self.choice_list() {
            if let Some(text) = value.as_text() {
                if !choices.iter().any(|choice| choice == text) {
                    return Err(SettingError::BadChoice {
                        setting: self.id.clone(),
                        choices: choices.to_vec(),
                        got: text.to_owned(),
                    });
                }
            }
        }
        if let Some(got) = numeric(value) {
            let min = self.min.unwrap_or(f64::NEG_INFINITY);
            let max = self.max.unwrap_or(f64::INFINITY);
            if got < min || got > max {
                return Err(SettingError::OutOfRange {
                    setting: self.id.clone(),
                    min,
                    max,
                    got,
                });
            }
        }
        Ok(())
    }
}

impl fmt::Display for Setting {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}) = {}", self.id, self.label, self.default.render())
    }
}

/// A change, with the value it was and the value it became.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingChange {
    pub setting: String,
    pub old: ConfigValue,
    pub new: ConfigValue,
}

/// A named set of values a mod applies in one go.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub values: BTreeMap<String, ConfigValue>,
}

impl Profile {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            values: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn set(mut self, id: &str, value: ConfigValue) -> Self {
        self.values.insert(id.to_owned(), value);
        self
    }
}

/// Every option a mod registered, as one schema.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingSchema {
    pub name: String,
    pub settings: Vec<Setting>,
}

impl SettingSchema {
    /// The defaults every option declares.
    #[must_use]
    pub fn defaults(&self) -> BTreeMap<String, ConfigValue> {
        self.settings
            .iter()
            .map(|setting| (setting.id.clone(), setting.default.clone()))
            .collect()
    }

    #[must_use]
    pub fn setting_for(&self, id: &str) -> Option<&Setting> {
        self.settings.iter().find(|setting| setting.id == id)
    }
}

type Watcher = Box<dyn FnMut(&SettingChange) + Send>;
type SetWatcher = Box<dyn FnMut(&BTreeMap<String, ConfigValue>) + Send>;

#[derive(Default)]
struct Held {
    settings: BTreeMap<String, Setting>,
    values: BTreeMap<String, ConfigValue>,
    watchers: Vec<(u64, String, Watcher)>,
    set_watchers: Vec<(u64, SetWatcher)>,
}

static NEXT_WATCHER: AtomicU64 = AtomicU64::new(1);

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(|| Mutex::new(Held::default()))
}

fn with<T>(body: impl FnOnce(&mut Held) -> T) -> T {
    let mut guard = held().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn numeric(value: &ConfigValue) -> Option<f64> {
    match value {
        ConfigValue::Integer(number) => Some(*number as f64),
        ConfigValue::Float(number) | ConfigValue::Percentage(number) => Some(*number),
        _ => None,
    }
}

/// Registers an option, whose value starts at its default. `false` when its id is taken.
pub fn register(setting: Setting) -> bool {
    events::register_event_type(EVENT_CHANGED);
    with(|held| {
        if held.settings.contains_key(&setting.id) {
            return false;
        }
        held.values
            .insert(setting.id.clone(), setting.default.clone());
        held.settings.insert(setting.id.clone(), setting);
        true
    })
}

/// Forgets an option and its value.
pub fn unregister(id: &str) -> bool {
    with(|held| {
        held.values.remove(id);
        held.settings.remove(id).is_some()
    })
}

/// An option by id, as it was registered.
#[must_use]
pub fn get_setting(id: &str) -> Option<Setting> {
    with(|held| held.settings.get(id).cloned())
}

/// What an option holds now.
#[must_use]
pub fn get(id: &str) -> Option<ConfigValue> {
    with(|held| {
        held.values
            .get(id)
            .cloned()
            .or_else(|| held.settings.get(id).map(|setting| setting.default.clone()))
    })
}

/// What an option was registered with.
#[must_use]
pub fn get_default(id: &str) -> Option<ConfigValue> {
    with(|held| held.settings.get(id).map(|setting| setting.default.clone()))
}

/// Every option's value, in id order.
#[must_use]
pub fn get_all() -> BTreeMap<String, ConfigValue> {
    with(|held| held.values.clone())
}

/// Every option a mod registered, as one schema.
#[must_use]
pub fn get_schema() -> SettingSchema {
    with(|held| SettingSchema {
        name: STORE.to_owned(),
        settings: held.settings.values().cloned().collect(),
    })
}

/// Every option in one category, in id order.
#[must_use]
pub fn by_category(category: &str) -> Vec<Setting> {
    with(|held| {
        held.settings
            .values()
            .filter(|setting| setting.category == category)
            .cloned()
            .collect()
    })
}

/// Every category an option declares, in name order.
#[must_use]
pub fn categories() -> Vec<String> {
    with(|held| {
        held.settings
            .values()
            .map(|setting| setting.category.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    })
}

/// Every option whose id or label contains `query`, ignoring case.
#[must_use]
pub fn search(query: &str) -> Vec<Setting> {
    let needle = query.to_lowercase();
    with(|held| {
        held.settings
            .values()
            .filter(|setting| {
                setting.id.to_lowercase().contains(&needle)
                    || setting.label.to_lowercase().contains(&needle)
            })
            .cloned()
            .collect()
    })
}

/// Whether `value` may be stored in `id`, without storing it.
pub fn validate_value(id: &str, value: &ConfigValue) -> Result<(), SettingError> {
    match get_setting(id) {
        Some(setting) => setting.validate(value),
        None => Err(SettingError::UnknownSetting {
            setting: id.to_owned(),
        }),
    }
}

/// What every option currently holds that it may not, as one line each.
#[must_use]
pub fn validate() -> Vec<String> {
    with(|held| {
        held.settings
            .values()
            .filter_map(|setting| {
                let value = held.values.get(&setting.id).unwrap_or(&setting.default);
                setting.validate(value).err().map(|error| error.to_string())
            })
            .collect()
    })
}

/// Sets an option's value, answering the change it made or `None` when it already held it.
pub fn set(id: &str, value: ConfigValue) -> Result<Option<SettingChange>, SettingError> {
    let change = with(|held| -> Result<Option<SettingChange>, SettingError> {
        let setting = held
            .settings
            .get(id)
            .cloned()
            .ok_or_else(|| SettingError::UnknownSetting {
                setting: id.to_owned(),
            })?;
        setting.validate(&value)?;
        let old = held
            .values
            .get(id)
            .cloned()
            .unwrap_or_else(|| setting.default.clone());
        if old == value {
            return Ok(None);
        }
        held.values.insert(id.to_owned(), value.clone());
        Ok(Some(SettingChange {
            setting: id.to_owned(),
            old,
            new: value,
        }))
    })?;
    if let Some(change) = &change {
        notify(change);
    }
    Ok(change)
}

/// Puts an option back to its default, answering the change it made.
pub fn reset(id: &str) -> Result<Option<SettingChange>, SettingError> {
    match get_default(id) {
        Some(default) => set(id, default),
        None => Err(SettingError::UnknownSetting {
            setting: id.to_owned(),
        }),
    }
}

/// Puts every option back to its default, and answers how many actually moved.
pub fn reset_all() -> usize {
    let ids: Vec<String> = with(|held| held.settings.keys().cloned().collect());
    ids.iter()
        .filter(|id| matches!(reset(id), Ok(Some(_))))
        .count()
}

/// The whole set as one [`ConfigValue`], the id of every option mapped to what it holds.
#[must_use]
pub fn export() -> ConfigValue {
    with(|held| {
        ConfigValue::Object(
            held.settings
                .iter()
                .map(|(id, setting)| {
                    let value = held
                        .values
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| setting.default.clone());
                    (id.clone(), value)
                })
                .collect(),
        )
    })
}

/// Reads a set back from [`export`], checking every value first, and answers how many went.
pub fn import(data: &ConfigValue) -> Result<usize, SettingError> {
    let fields = data.as_map().ok_or_else(|| SettingError::NotASet {
        found: data.kind().as_str().to_owned(),
    })?;
    for (id, value) in fields {
        let setting = get_setting(id).ok_or_else(|| SettingError::UnknownSetting {
            setting: id.clone(),
        })?;
        setting.validate(value)?;
    }
    let mut count = 0;
    for (id, value) in fields {
        set(id, value.clone())?;
        count += 1;
    }
    Ok(count)
}

/// Applies a named set, checking every value first, and answers the changes it made.
pub fn apply(profile: &Profile) -> Result<Vec<SettingChange>, SettingError> {
    for (id, value) in &profile.values {
        let setting = get_setting(id).ok_or_else(|| SettingError::UnknownSetting {
            setting: id.clone(),
        })?;
        setting.validate(value)?;
    }
    let mut changes = Vec::new();
    for (id, value) in &profile.values {
        if let Some(change) = set(id, value.clone())? {
            changes.push(change);
        }
    }
    Ok(changes)
}

/// Writes the whole set to its own persistence store, and answers whether it went.
pub fn save() -> bool {
    if let Err(error) = crate::persistence::open(STORE) {
        crate::warn!("a mod could not open its settings store: {error}");
        return false;
    }
    for (id, value) in get_all() {
        crate::persistence::set(&id, value);
    }
    let saved = crate::persistence::save();
    crate::persistence::close();
    saved
}

/// Reads the set back from its own persistence store, and answers whether it read one.
pub fn load() -> bool {
    if let Err(error) = crate::persistence::open(STORE) {
        crate::warn!("a mod could not open its settings store: {error}");
        return false;
    }
    let stored = crate::persistence::get_all();
    crate::persistence::close();
    let mut count = 0;
    for (id, value) in stored {
        if get_setting(&id).is_some() && set(&id, value).is_ok() {
            count += 1;
        }
    }
    count > 0
}

/// Calls `watcher` whenever `id` changes. Returns an id for [`unwatch`].
pub fn watch(id: &str, watcher: impl FnMut(&SettingChange) + Send + 'static) -> u64 {
    events::register_event_type(EVENT_CHANGED);
    let id_num = NEXT_WATCHER.fetch_add(1, Ordering::Relaxed);
    with(|held| held.watchers.push((id_num, id.to_owned(), Box::new(watcher))));
    id_num
}

/// Calls `watcher` with the whole set whenever anything changes. Returns an id for [`unwatch`].
pub fn on_changed(watcher: impl FnMut(&BTreeMap<String, ConfigValue>) + Send + 'static) -> u64 {
    events::register_event_type(EVENT_CHANGED);
    let id = NEXT_WATCHER.fetch_add(1, Ordering::Relaxed);
    with(|held| held.set_watchers.push((id, Box::new(watcher))));
    id
}

/// Stops a watcher of either kind, and answers whether it existed.
pub fn unwatch(id: u64) -> bool {
    with(|held| {
        let before = held.watchers.len() + held.set_watchers.len();
        held.watchers.retain(|(held, _, _)| *held != id);
        held.set_watchers.retain(|(held, _)| *held != id);
        held.watchers.len() + held.set_watchers.len() != before
    })
}

fn notify(change: &SettingChange) {
    with(|held| {
        for (_, watched, watcher) in &mut held.watchers {
            if watched == &change.setting {
                watcher(change);
            }
        }
        let all = held.values.clone();
        for (_, watcher) in &mut held.set_watchers {
            watcher(&all);
        }
    });
    let text = format!("{} -> {}", change.old.render(), change.new.render());
    events::publish(Event::new(EVENT_CHANGED).from(&change.setting).with_text(&text));
}

/// Why a question about the client's own options has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "video" => {
            "the client draws its video options from its own render pipeline, so reading or \
             changing one is a change to the running client and needs a hook this host has"
                .to_owned()
        }
        "audio" => {
            "the client mixes its audio inside its own sound engine, which this host never \
             sees, so the master volume and the device stay the client's"
                .to_owned()
        }
        "controls" => {
            "the client keeps its key bindings and mouse buttons in its own input map, which \
             this host cannot read or rebind"
                .to_owned()
        }
        "accessibility" => {
            "the client's accessibility options change how its own screens draw and read, a \
             layer this host does not reach"
                .to_owned()
        }
        "language" => {
            "the client chooses its language from its own resource set at startup, which this \
             host can neither read nor switch"
                .to_owned()
        }
        "render_distance" => {
            "the client's render distance sizes its own chunk render pipeline, which a mod \
             here cannot touch"
                .to_owned()
        }
        "simulation_distance" => {
            "the client's simulation distance is the world's own ticking radius, decided \
             between client and server and not by this host"
                .to_owned()
        }
        "fov" => {
            "the client's field of view is its camera projection, rebuilt every frame by code \
             this host does not hook"
                .to_owned()
        }
        "sensitivity" => {
            "the client's mouse sensitivity scales its own look input before a mod ever sees \
             it"
                .to_owned()
        }
        "mouse_invert" => {
            "the client inverts its own mouse axis in its input reader, a flag this host \
             cannot see"
                .to_owned()
        }
        "particles" => {
            "the client's particle option gates its own particle engine, which this host \
             cannot reach"
                .to_owned()
        }
        "graphics_mode" => {
            "the client's graphics mode chooses its own render quality, which a mod here \
             cannot read or set"
                .to_owned()
        }
        "fullscreen" => {
            "the client's fullscreen flag belongs to its own window, which the platform layer \
             owns and not this host"
                .to_owned()
        }
        "resolution" => {
            "the client's resolution is its own window's framebuffer size, chosen by the \
             platform and invisible here"
                .to_owned()
        }
        "vsync" => {
            "the client's vsync is a swap-chain option of its own window, which this host does \
             not control"
                .to_owned()
        }
        "frame_limit" => {
            "the client's frame limit is applied by its own frame loop, which this host only \
             observes"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The client options this host cannot reach. Each one answers with its own sentence.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "video",
        "audio",
        "controls",
        "accessibility",
        "language",
        "render_distance",
        "simulation_distance",
        "fov",
        "sensitivity",
        "mouse_invert",
        "particles",
        "graphics_mode",
        "fullscreen",
        "resolution",
        "vsync",
        "frame_limit",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

fn refuse(what: &str) {
    crate::warn!("a mod asked this host about the client's options: {}", why_unknown(what));
}

/// The client's video options. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_video() -> Option<ConfigValue> {
    refuse("video");
    None
}

/// The client's audio options. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_audio() -> Option<ConfigValue> {
    refuse("audio");
    None
}

/// The client's controls. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_controls() -> Option<ConfigValue> {
    refuse("controls");
    None
}

/// The client's accessibility options. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_accessibility() -> Option<ConfigValue> {
    refuse("accessibility");
    None
}

/// The client's language. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_language() -> Option<ConfigValue> {
    refuse("language");
    None
}

/// The client's render distance. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_render_distance() -> Option<ConfigValue> {
    refuse("render_distance");
    None
}

/// The client's simulation distance. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_simulation_distance() -> Option<ConfigValue> {
    refuse("simulation_distance");
    None
}

/// The client's field of view. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_fov() -> Option<ConfigValue> {
    refuse("fov");
    None
}

/// The client's mouse sensitivity. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_sensitivity() -> Option<ConfigValue> {
    refuse("sensitivity");
    None
}

/// Whether the client inverts its mouse. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_mouse_invert() -> Option<ConfigValue> {
    refuse("mouse_invert");
    None
}

/// The client's particle option. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_particles() -> Option<ConfigValue> {
    refuse("particles");
    None
}

/// The client's graphics mode. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_graphics_mode() -> Option<ConfigValue> {
    refuse("graphics_mode");
    None
}

/// Whether the client is fullscreen. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_fullscreen() -> Option<ConfigValue> {
    refuse("fullscreen");
    None
}

/// The client's resolution. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_resolution() -> Option<ConfigValue> {
    refuse("resolution");
    None
}

/// Whether the client uses vsync. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_vsync() -> Option<ConfigValue> {
    refuse("vsync");
    None
}

/// The client's frame limit. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_frame_limit() -> Option<ConfigValue> {
    refuse("frame_limit");
    None
}

/// Changes one of the client's own options. `false`, always: [`why_unknown`] says why.
pub fn set_client(what: &str, _value: ConfigValue) -> bool {
    refuse(what);
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
        with(|held| {
            held.settings.clear();
            held.values.clear();
            held.watchers.clear();
            held.set_watchers.clear();
        });
        events::unsubscribe_all(EVENT_CHANGED);
    }

    fn distance() -> Setting {
        Setting::new("hello:distance", ConfigType::Integer, ConfigValue::Integer(8))
            .label("Draw distance")
            .tooltip("chunks to draw")
            .category("hello:display")
            .bounded(2.0, 32.0)
    }

    #[test]
    fn an_option_is_registered_read_back_and_a_duplicate_is_refused() {
        let _serial = serial();
        fresh();
        assert!(register(distance()));
        assert!(
            !register(Setting::new(
                "hello:distance",
                ConfigType::Integer,
                ConfigValue::Integer(4),
            )),
            "a second option with the same id is refused"
        );
        assert_eq!(get_setting("hello:distance"), Some(distance()));
        assert_eq!(get("hello:distance"), Some(ConfigValue::Integer(8)));
        assert_eq!(get_default("hello:distance"), Some(ConfigValue::Integer(8)));
        assert_eq!(get_all().len(), 1);

        register(
            Setting::new("hello:volume", ConfigType::Float, ConfigValue::Float(0.5))
                .label("Volume")
                .category("hello:audio"),
        );
        let schema = get_schema();
        assert_eq!(schema.settings.len(), 2);
        assert_eq!(schema.defaults().len(), 2);
        assert_eq!(
            schema.setting_for("hello:volume").expect("registered").label,
            "Volume"
        );
        assert_eq!(by_category("hello:display").len(), 1);
        assert_eq!(by_category("hello:display")[0].id, "hello:distance");
        assert_eq!(categories(), vec!["hello:audio".to_owned(), "hello:display".to_owned()]);
        assert_eq!(search("draw")[0].id, "hello:distance");
        assert_eq!(search("volume")[0].id, "hello:volume");
        assert!(search("nothing").is_empty());
        assert!(unregister("hello:distance"));
        assert!(!unregister("hello:distance"));
    }

    #[test]
    fn a_value_of_the_wrong_kind_or_off_the_list_is_refused_with_what_it_wanted() {
        let _serial = serial();
        fresh();
        register(distance());
        assert_eq!(
            set("hello:distance", ConfigValue::Text("far".to_owned())),
            Err(SettingError::WrongKind {
                setting: "hello:distance".to_owned(),
                wanted: "integer".to_owned(),
                found: "text".to_owned(),
            })
        );
        assert_eq!(get("hello:distance"), Some(ConfigValue::Integer(8)));
        register(
            Setting::new("hello:mode", ConfigType::Enum, ConfigValue::Enum("easy".to_owned()))
                .choices(&["easy", "hard"]),
        );
        assert_eq!(
            set("hello:mode", ConfigValue::Enum("wild".to_owned())),
            Err(SettingError::BadChoice {
                setting: "hello:mode".to_owned(),
                choices: vec!["easy".to_owned(), "hard".to_owned()],
                got: "wild".to_owned(),
            })
        );
        assert!(set("hello:mode", ConfigValue::Enum("hard".to_owned()))
            .expect("valid")
            .is_some());
    }

    #[test]
    fn a_value_outside_the_bounds_is_refused_with_the_bounds() {
        let _serial = serial();
        fresh();
        register(distance());
        assert_eq!(
            set("hello:distance", ConfigValue::Integer(99)),
            Err(SettingError::OutOfRange {
                setting: "hello:distance".to_owned(),
                min: 2.0,
                max: 32.0,
                got: 99.0,
            })
        );
        assert_eq!(
            validate_value("hello:distance", &ConfigValue::Integer(1)),
            Err(SettingError::OutOfRange {
                setting: "hello:distance".to_owned(),
                min: 2.0,
                max: 32.0,
                got: 1.0,
            })
        );
        assert!(validate_value("hello:distance", &ConfigValue::Integer(32)).is_ok());
        assert!(validate().is_empty());
    }

    #[test]
    fn a_change_fires_the_watcher_and_the_bus_once_and_a_same_value_does_not() {
        let _serial = serial();
        fresh();
        register(distance());
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&heard);
        let watcher = watch("hello:distance", move |change| {
            into.lock()
                .unwrap()
                .push(format!("{} {}->{}", change.setting, change.old.render(), change.new.render()));
        });
        let all = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&all);
        let set_watch = on_changed(move |values| {
            seen.lock().unwrap().push(values.len());
        });
        let bus = Arc::new(Mutex::new(Vec::new()));
        let on_bus = Arc::clone(&bus);
        events::subscribe(EVENT_CHANGED, events::Priority::Normal, move |event| {
            on_bus.lock().unwrap().push(event.text().unwrap_or_default().to_owned());
        });

        let change = set("hello:distance", ConfigValue::Integer(12))
            .expect("valid")
            .expect("it changed");
        assert_eq!(change.old, ConfigValue::Integer(8));
        assert_eq!(change.new, ConfigValue::Integer(12));
        assert_eq!(
            set("hello:distance", ConfigValue::Integer(12)).expect("valid"),
            None,
            "setting the value it already holds fires nothing"
        );
        assert_eq!(*heard.lock().unwrap(), vec!["hello:distance 8->12".to_owned()]);
        assert_eq!(*all.lock().unwrap(), vec![1], "the whole set is handed to the watcher");
        assert_eq!(*bus.lock().unwrap(), vec!["8 -> 12".to_owned()]);
        assert!(events::is_registered_kind(EVENT_CHANGED));
        assert!(unwatch(watcher));
        assert!(unwatch(set_watch));
        assert!(!unwatch(watcher));
    }

    #[test]
    fn reset_and_reset_all_put_options_back_to_their_defaults() {
        let _serial = serial();
        fresh();
        register(distance());
        register(
            Setting::new("hello:volume", ConfigType::Float, ConfigValue::Float(0.5))
                .category("hello:audio"),
        );
        set("hello:distance", ConfigValue::Integer(20)).expect("valid");
        set("hello:volume", ConfigValue::Float(0.9)).expect("valid");

        let change = reset("hello:distance").expect("known").expect("it changed");
        assert_eq!(change.old, ConfigValue::Integer(20));
        assert_eq!(change.new, ConfigValue::Integer(8));
        assert_eq!(reset("hello:distance").expect("known"), None, "already at its default");
        set("hello:distance", ConfigValue::Integer(20)).expect("valid");
        assert_eq!(reset_all(), 2, "both moved back");
        assert_eq!(get("hello:distance"), Some(ConfigValue::Integer(8)));
        assert_eq!(get("hello:volume"), Some(ConfigValue::Float(0.5)));
        assert_eq!(reset_all(), 0, "nothing moves twice");
        assert_eq!(
            reset("nothing").unwrap_err(),
            SettingError::UnknownSetting {
                setting: "nothing".to_owned()
            }
        );
    }

    #[test]
    fn the_whole_set_round_trips_through_the_config_form() {
        let _serial = serial();
        fresh();
        register(distance());
        register(
            Setting::new("hello:volume", ConfigType::Float, ConfigValue::Float(0.5))
                .category("hello:audio"),
        );
        set("hello:distance", ConfigValue::Integer(16)).expect("valid");
        let snapshot = export();
        let _ = reset_all();
        assert_eq!(import(&snapshot), Ok(2));
        assert_eq!(export(), snapshot, "the round trip is exact");
        assert_eq!(
            import(&ConfigValue::Boolean(true)).unwrap_err(),
            SettingError::NotASet {
                found: "boolean".to_owned()
            }
        );
        let unknown = ConfigValue::Object(BTreeMap::from([(
            "nothing".to_owned(),
            ConfigValue::Integer(1),
        )]));
        assert_eq!(
            import(&unknown).unwrap_err(),
            SettingError::UnknownSetting {
                setting: "nothing".to_owned()
            }
        );
    }

    #[test]
    fn a_profile_applies_and_reports_what_it_changed() {
        let _serial = serial();
        fresh();
        register(distance());
        register(
            Setting::new("hello:volume", ConfigType::Float, ConfigValue::Float(0.5))
                .category("hello:audio"),
        );
        let quiet = Profile::new("quiet")
            .set("hello:distance", ConfigValue::Integer(12))
            .set("hello:volume", ConfigValue::Float(0.5));
        let changes = apply(&quiet).expect("every value fits");
        assert_eq!(changes.len(), 1, "only the value that moved is reported");
        assert_eq!(changes[0].setting, "hello:distance");
        assert_eq!(changes[0].new, ConfigValue::Integer(12));
        let noisy = Profile::new("noisy").set("nothing", ConfigValue::Integer(1));
        assert_eq!(
            apply(&noisy).unwrap_err(),
            SettingError::UnknownSetting {
                setting: "nothing".to_owned()
            }
        );
    }

    #[test]
    fn the_whole_set_saves_and_loads_through_persistence_and_the_client_is_refused() {
        let _serial = serial();
        fresh();
        let root = std::env::temp_dir().join(format!(
            "cinnaloader-settings-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        crate::fs::set_root(root.clone());

        register(distance());
        set("hello:distance", ConfigValue::Integer(16)).expect("valid");
        assert!(save(), "the set is written to its own store");
        set("hello:distance", ConfigValue::Integer(4)).expect("valid");
        assert!(load(), "the set reads back");
        assert_eq!(get("hello:distance"), Some(ConfigValue::Integer(16)));
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(refusals().len(), 16);
        let reasons: BTreeSet<String> = refusals().into_iter().map(|(_, why)| why).collect();
        assert_eq!(reasons.len(), 16, "every kind has its own sentence");
        assert!(why_unknown("video").contains("video options"));
        assert!(why_unknown("frame_limit").contains("frame loop"));
        assert_eq!(why_unknown("nonsense"), "nonsense is not something this host can see");
        assert!(get_video().is_none());
        assert!(get_audio().is_none());
        assert!(get_controls().is_none());
        assert!(get_accessibility().is_none());
        assert!(get_language().is_none());
        assert!(get_render_distance().is_none());
        assert!(get_simulation_distance().is_none());
        assert!(get_fov().is_none());
        assert!(get_sensitivity().is_none());
        assert!(get_mouse_invert().is_none());
        assert!(get_particles().is_none());
        assert!(get_graphics_mode().is_none());
        assert!(get_fullscreen().is_none());
        assert!(get_resolution().is_none());
        assert!(get_vsync().is_none());
        assert!(get_frame_limit().is_none());
        assert!(!set_client("video", ConfigValue::Boolean(true)));
    }
}
