//! Accessibility: the option set a mod owns, the palettes and captions it can put in front of a
//! player, and the client's own accessibility layer this host cannot reach.
//!
//! An **option** is a value a mod ships, the same shape [`crate::settings`] uses: an id, a kind,
//! a default, optional bounds, a label. [`set_option`] checks the kind and the bounds and answers
//! a real error when either is wrong; a change fires the watchers and the §6 bus with the old and
//! the new value, and fires nothing when the value did not move. [`get_settings`] reads the whole
//! set as one struct and [`set_settings`] applies one, reporting only what moved.
//!
//! A [`ColorPalette`] is a named set of colour replacements with a real lookup — a colour it does
//! not list is returned unchanged — and [`ColorPalette::apply`] rewrites a [`crate::chat`] message
//! through it. A palette can be [`register_color_alternative`]ed, selected, and listed, and
//! selecting one is a change to the option set.
//!
//! Captions are a queue a mod owns: [`show_caption`] queues a line with its duration and source,
//! [`advance_captions`] expires each one at exactly its duration, and [`caption_at`] answers what
//! is showing at a moment. The loader's own overlay is the one place a line can actually be
//! shown, so the queue is what a mod hands it.
//!
//! Contrast is real arithmetic: [`relative_luminance`] and [`contrast_ratio`] are the WCAG
//! formula, and [`is_readable`] answers against the 4.5 threshold.
//!
//! What this host cannot do is touch the client's own accessibility. Its options change how its
//! own screens draw and read, its key repeat and input assistance live in its own input layer, and
//! its screen reader is what speaks a line. Reading one, or announcing through it, is refused with
//! a reason in [`why_unknown`], listed by [`refusals`], and never answered with a fake success.
//!
//! ```
//! use cinnaloader::accessibility as a11y;
//! use cinnaloader::chat::TextColor;
//!
//! let palette = a11y::ColorPalette::new("hello:protanopia")
//!     .replace(TextColor::named("red"), TextColor::named("blue"));
//! assert_eq!(palette.lookup(&TextColor::named("red")), TextColor::named("blue"));
//! assert_eq!(palette.lookup(&TextColor::named("green")), TextColor::named("green"));
//!
//! assert_eq!(a11y::get_widget_description("slider", "Volume"), Some("Volume, slider".to_owned()));
//! assert_eq!(a11y::contrast_ratio((0, 0, 0), (255, 255, 255)), 21.0);
//! assert!(!a11y::announce("hello"), "the client's screen reader is its own");
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::chat::{Message, TextColor};
use crate::config::{ConfigType, ConfigValue};
use crate::events::{self, Event};

/// The §6 kind fired when an accessibility option changes.
pub const EVENT_CHANGED: &str = "accessibility_changed";

/// The option that names the selected colour-blind palette.
pub const PALETTE_OPTION: &str = "color_blind_palette";

/// The contrast ratio a pair of colours must reach to count as readable.
pub const READABLE_RATIO: f64 = 4.5;

/// Why an option was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionError {
    WrongKind {
        option: String,
        wanted: String,
        found: String,
    },
    OutOfRange {
        option: String,
        min: f64,
        max: f64,
        got: f64,
    },
    BadChoice {
        option: String,
        choices: Vec<String>,
        got: String,
    },
    Unknown {
        option: String,
    },
}

impl fmt::Display for OptionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongKind { option, wanted, found } => {
                write!(formatter, "{option} wants {wanted}, got {found}")
            }
            Self::OutOfRange { option, min, max, got } => {
                write!(formatter, "{option} is {got}, outside {min}..{max}")
            }
            Self::BadChoice { option, choices, got } => {
                write!(formatter, "{option} is {got}, not one of {}", choices.join(", "))
            }
            Self::Unknown { option } => {
                write!(formatter, "{option} is not a registered accessibility option")
            }
        }
    }
}

/// One accessibility option a mod owns: what it holds and what it may hold.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessOption {
    pub id: String,
    pub kind: ConfigType,
    pub default: ConfigValue,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub label: String,
}

impl AccessOption {
    #[must_use]
    pub fn new(id: &str, kind: ConfigType, default: ConfigValue) -> Self {
        Self {
            id: id.to_owned(),
            kind,
            default,
            min: None,
            max: None,
            label: id.to_owned(),
        }
    }

    #[must_use]
    pub fn bounded(mut self, min: f64, max: f64) -> Self {
        self.min = Some(min);
        self.max = Some(max);
        self
    }

    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = label.to_owned();
        self
    }

    /// Whether `value` fits this option's kind and bounds, and why not when it may not.
    pub fn validate(&self, value: &ConfigValue) -> Result<(), OptionError> {
        if !self.kind.accepts(value.kind()) {
            return Err(OptionError::WrongKind {
                option: self.id.clone(),
                wanted: self.kind.as_str().to_owned(),
                found: value.kind().as_str().to_owned(),
            });
        }
        if let Some(got) = numeric(value) {
            let min = self.min.unwrap_or(f64::NEG_INFINITY);
            let max = self.max.unwrap_or(f64::INFINITY);
            if got < min || got > max {
                return Err(OptionError::OutOfRange {
                    option: self.id.clone(),
                    min,
                    max,
                    got,
                });
            }
        }
        Ok(())
    }
}

/// A change, with the value it was and the value it became.
#[derive(Debug, Clone, PartialEq)]
pub struct OptionChange {
    pub option: String,
    pub old: ConfigValue,
    pub new: ConfigValue,
}

/// The whole option set, typed, as a mod reads it in one look.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessibilitySettings {
    pub reduced_motion: bool,
    pub ui_scale: f64,
    pub text_scale: f64,
    pub subtitles: bool,
    pub screen_reader: bool,
    pub high_contrast: bool,
    pub color_blind_palette: String,
}

impl Default for AccessibilitySettings {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            ui_scale: 1.0,
            text_scale: 1.0,
            subtitles: false,
            screen_reader: false,
            high_contrast: false,
            color_blind_palette: String::new(),
        }
    }
}

/// A named set of colour replacements, source colour to the one drawn instead.
#[derive(Debug, Clone, PartialEq)]
pub struct ColorPalette {
    pub name: String,
    pub replacements: BTreeMap<String, TextColor>,
}

impl ColorPalette {
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            replacements: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn replace(mut self, from: TextColor, to: TextColor) -> Self {
        self.replacements.insert(from.as_str(), to);
        self
    }

    /// What `color` is drawn as here. A colour not listed is returned unchanged.
    #[must_use]
    pub fn lookup(&self, color: &TextColor) -> TextColor {
        self.replacements
            .get(&color.as_str())
            .cloned()
            .unwrap_or_else(|| color.clone())
    }

    /// `message` with every colour it sets replaced, children and all.
    #[must_use]
    pub fn apply(&self, message: &Message) -> Message {
        let mut out = message.clone();
        if let Some(color) = &out.style.color {
            out.style.color = Some(self.lookup(color));
        }
        out.children = out.children.iter().map(|child| self.apply(child)).collect();
        out
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.replacements.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.replacements.is_empty()
    }
}

/// One thing a mod wants a screen reader to reach by name.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessibleWidget {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub description: Option<String>,
}

impl AccessibleWidget {
    #[must_use]
    pub fn new(id: &str, kind: &str, label: &str) -> Self {
        Self {
            id: id.to_owned(),
            kind: kind.to_owned(),
            label: label.to_owned(),
            description: None,
        }
    }

    #[must_use]
    pub fn described(mut self, description: &str) -> Self {
        self.description = Some(description.to_owned());
        self
    }
}

/// One caption line and how long it stays on screen.
#[derive(Debug, Clone, PartialEq)]
pub struct Caption {
    pub text: String,
    pub duration: f64,
    pub source: String,
    pub start: f64,
}

impl Caption {
    /// The moment it stops showing, its start plus exactly its duration.
    #[must_use]
    pub fn end(&self) -> f64 {
        self.start + self.duration
    }
}

type Watcher = Box<dyn FnMut(&OptionChange) + Send>;

#[derive(Default)]
struct Held {
    values: BTreeMap<String, ConfigValue>,
    watchers: Vec<(u64, Watcher)>,
    palettes: BTreeMap<String, ColorPalette>,
    widgets: BTreeMap<String, AccessibleWidget>,
    narrations: BTreeMap<String, String>,
    visuals: BTreeMap<String, String>,
    focus: Vec<String>,
    captions: Vec<Caption>,
    now: f64,
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

fn schema() -> Vec<AccessOption> {
    vec![
        AccessOption::new("reduced_motion", ConfigType::Boolean, ConfigValue::Boolean(false))
            .label("Reduced motion"),
        AccessOption::new("ui_scale", ConfigType::Float, ConfigValue::Float(1.0))
            .bounded(0.5, 4.0)
            .label("UI scale"),
        AccessOption::new("text_scale", ConfigType::Float, ConfigValue::Float(1.0))
            .bounded(0.5, 4.0)
            .label("Text size"),
        AccessOption::new("subtitles", ConfigType::Boolean, ConfigValue::Boolean(false))
            .label("Subtitles"),
        AccessOption::new("screen_reader", ConfigType::Boolean, ConfigValue::Boolean(false))
            .label("Screen reader"),
        AccessOption::new("high_contrast", ConfigType::Boolean, ConfigValue::Boolean(false))
            .label("High contrast"),
        AccessOption::new(PALETTE_OPTION, ConfigType::Text, ConfigValue::Text(String::new()))
            .label("Colour-blind palette"),
    ]
}

fn option_for(id: &str) -> Option<AccessOption> {
    schema().into_iter().find(|option| option.id == id)
}

fn numeric(value: &ConfigValue) -> Option<f64> {
    match value {
        ConfigValue::Integer(number) => Some(*number as f64),
        ConfigValue::Float(number) | ConfigValue::Percentage(number) => Some(*number),
        _ => None,
    }
}

fn value_of(held: &Held, id: &str) -> Option<ConfigValue> {
    held.values
        .get(id)
        .cloned()
        .or_else(|| option_for(id).map(|option| option.default))
}

fn as_bool(value: Option<ConfigValue>) -> bool {
    matches!(value, Some(ConfigValue::Boolean(true)))
}

fn as_float(value: Option<ConfigValue>) -> f64 {
    match value {
        Some(ConfigValue::Float(number)) => number,
        Some(ConfigValue::Integer(number)) => number as f64,
        _ => 0.0,
    }
}

fn as_text(value: Option<ConfigValue>) -> String {
    match value {
        Some(ConfigValue::Text(text)) | Some(ConfigValue::String(text)) => text,
        _ => String::new(),
    }
}

/// Every accessibility option a mod owns, as one schema.
#[must_use]
pub fn options() -> Vec<AccessOption> {
    schema()
}

/// An option by id, as it was declared.
#[must_use]
pub fn get_option(id: &str) -> Option<AccessOption> {
    option_for(id)
}

/// What an option holds now, or its default when it was never set.
#[must_use]
pub fn get_option_value(id: &str) -> Option<ConfigValue> {
    with(|held| value_of(held, id))
}

fn validate(id: &str, value: &ConfigValue) -> Result<(), OptionError> {
    let option = option_for(id).ok_or_else(|| OptionError::Unknown {
        option: id.to_owned(),
    })?;
    option.validate(value)?;
    if id == PALETTE_OPTION {
        if let Some(name) = value.as_text() {
            if !name.is_empty() && !with(|held| held.palettes.contains_key(name)) {
                return Err(OptionError::BadChoice {
                    option: id.to_owned(),
                    choices: with(|held| held.palettes.keys().cloned().collect()),
                    got: name.to_owned(),
                });
            }
        }
    }
    Ok(())
}

/// Sets an option's value, answering the change it made or `None` when it already held it.
pub fn set_option(id: &str, value: ConfigValue) -> Result<Option<OptionChange>, OptionError> {
    validate(id, &value)?;
    let change = with(|held| {
        let default = option_for(id).expect("validated").default;
        let old = value_of(held, id).unwrap_or(default);
        if old == value {
            return None;
        }
        held.values.insert(id.to_owned(), value.clone());
        Some(OptionChange {
            option: id.to_owned(),
            old,
            new: value,
        })
    });
    if let Some(change) = &change {
        notify(change);
    }
    Ok(change)
}

/// The whole option set, typed.
#[must_use]
pub fn get_settings() -> AccessibilitySettings {
    with(|held| AccessibilitySettings {
        reduced_motion: as_bool(value_of(held, "reduced_motion")),
        ui_scale: as_float(value_of(held, "ui_scale")),
        text_scale: as_float(value_of(held, "text_scale")),
        subtitles: as_bool(value_of(held, "subtitles")),
        screen_reader: as_bool(value_of(held, "screen_reader")),
        high_contrast: as_bool(value_of(held, "high_contrast")),
        color_blind_palette: as_text(value_of(held, PALETTE_OPTION)),
    })
}

/// Applies a whole option set, checking every value first, and answers the changes it made.
pub fn set_settings(settings: &AccessibilitySettings) -> Result<Vec<OptionChange>, OptionError> {
    let pairs = [
        ("reduced_motion", ConfigValue::Boolean(settings.reduced_motion)),
        ("ui_scale", ConfigValue::Float(settings.ui_scale)),
        ("text_scale", ConfigValue::Float(settings.text_scale)),
        ("subtitles", ConfigValue::Boolean(settings.subtitles)),
        ("screen_reader", ConfigValue::Boolean(settings.screen_reader)),
        ("high_contrast", ConfigValue::Boolean(settings.high_contrast)),
        (
            PALETTE_OPTION,
            ConfigValue::Text(settings.color_blind_palette.clone()),
        ),
    ];
    for (id, value) in &pairs {
        validate(id, value)?;
    }
    let mut changes = Vec::new();
    for (id, value) in pairs {
        if let Some(change) = set_option(id, value)? {
            changes.push(change);
        }
    }
    Ok(changes)
}

/// Whether the mod asks for reduced motion.
#[must_use]
pub fn get_reduced_motion() -> bool {
    as_bool(get_option_value("reduced_motion"))
}

/// The mod's UI scale.
#[must_use]
pub fn get_ui_scale() -> f64 {
    as_float(get_option_value("ui_scale"))
}

/// The mod's text size.
#[must_use]
pub fn get_text_scale() -> f64 {
    as_float(get_option_value("text_scale"))
}

/// Whether the mod wants high contrast.
#[must_use]
pub fn get_contrast_mode() -> bool {
    as_bool(get_option_value("high_contrast"))
}

/// The colour-blind palette the mod selected, empty when it selected none.
#[must_use]
pub fn get_color_filters() -> String {
    as_text(get_option_value(PALETTE_OPTION))
}

/// Whether the mod wants captions shown.
#[must_use]
pub fn get_subtitles_enabled() -> bool {
    as_bool(get_option_value("subtitles"))
}

/// Whether the mod asks a screen reader to speak its text.
#[must_use]
pub fn get_screen_reader_enabled() -> bool {
    as_bool(get_option_value("screen_reader"))
}

/// Calls `watcher` whenever an option changes. Returns an id for [`unwatch`].
pub fn watch(watcher: impl FnMut(&OptionChange) + Send + 'static) -> u64 {
    events::register_event_type(EVENT_CHANGED);
    let id = NEXT_WATCHER.fetch_add(1, Ordering::Relaxed);
    with(|held| held.watchers.push((id, Box::new(watcher))));
    id
}

/// Stops a watcher, and answers whether it existed.
pub fn unwatch(id: u64) -> bool {
    with(|held| {
        let before = held.watchers.len();
        held.watchers.retain(|(held_id, _)| *held_id != id);
        held.watchers.len() != before
    })
}

fn notify(change: &OptionChange) {
    with(|held| {
        for (_, watcher) in &mut held.watchers {
            watcher(change);
        }
    });
    events::register_event_type(EVENT_CHANGED);
    let text = format!("{} -> {}", change.old.render(), change.new.render());
    events::publish(Event::new(EVENT_CHANGED).from(&change.option).with_text(&text));
}

/// Registers a palette, whose name must be free. `false` when it is taken.
pub fn register_color_alternative(palette: ColorPalette) -> bool {
    with(|held| {
        if held.palettes.contains_key(&palette.name) {
            return false;
        }
        held.palettes.insert(palette.name.clone(), palette);
        true
    })
}

/// A registered palette by name.
#[must_use]
pub fn get_color_alternative(name: &str) -> Option<ColorPalette> {
    with(|held| held.palettes.get(name).cloned())
}

/// Every registered palette name, in name order.
#[must_use]
pub fn list_color_alternatives() -> Vec<String> {
    with(|held| held.palettes.keys().cloned().collect())
}

/// Selects a registered palette, which is a change to [`PALETTE_OPTION`]. An empty name clears it.
pub fn select_color_alternative(name: &str) -> Result<Option<OptionChange>, OptionError> {
    set_option(PALETTE_OPTION, ConfigValue::Text(name.to_owned()))
}

/// Describes a UI element for a screen reader from its kind and label.
#[must_use]
pub fn get_widget_description(kind: &str, label: &str) -> Option<String> {
    let kind = kind.trim();
    let label = label.trim();
    if kind.is_empty() && label.is_empty() {
        return None;
    }
    if label.is_empty() {
        return Some(kind.to_owned());
    }
    match kind {
        "" | "label" | "text" => Some(label.to_owned()),
        _ => Some(format!("{label}, {kind}")),
    }
}

/// Registers a widget a screen reader should reach. `false` when its id is taken.
pub fn register_accessible_widget(widget: AccessibleWidget) -> bool {
    with(|held| {
        if held.widgets.contains_key(&widget.id) {
            return false;
        }
        held.widgets.insert(widget.id.clone(), widget);
        true
    })
}

/// A registered widget by id.
#[must_use]
pub fn get_accessible_widget(id: &str) -> Option<AccessibleWidget> {
    with(|held| held.widgets.get(id).cloned())
}

/// Every registered widget id, in id order.
#[must_use]
pub fn list_accessible_widgets() -> Vec<String> {
    with(|held| held.widgets.keys().cloned().collect())
}

/// Forgets a widget. `false` when there was none.
pub fn unregister_accessible_widget(id: &str) -> bool {
    with(|held| held.widgets.remove(id).is_some())
}

/// Sets a registered widget's own description. `false` when it is not registered.
pub fn set_widget_description(id: &str, description: &str) -> bool {
    with(|held| match held.widgets.get_mut(id) {
        Some(widget) => {
            widget.description = Some(description.to_owned());
            true
        }
        None => false,
    })
}

/// Registers narration text under an id. `false` when the id is empty or taken.
pub fn register_narration(id: &str, text: &str) -> bool {
    with(|held| {
        if id.is_empty() || held.narrations.contains_key(id) {
            return false;
        }
        held.narrations.insert(id.to_owned(), text.to_owned());
        true
    })
}

/// Registered narration text by id.
#[must_use]
pub fn get_narration(id: &str) -> Option<String> {
    with(|held| held.narrations.get(id).cloned())
}

/// Every narration id, in id order.
#[must_use]
pub fn list_narrations() -> Vec<String> {
    with(|held| held.narrations.keys().cloned().collect())
}

/// Forgets narration text. `false` when there was none.
pub fn unregister_narration(id: &str) -> bool {
    with(|held| held.narrations.remove(id).is_some())
}

/// Registers a visual alternative for an event. `false` when the event is empty or taken.
pub fn register_visual_alternative(event: &str, text: &str) -> bool {
    with(|held| {
        if event.is_empty() || held.visuals.contains_key(event) {
            return false;
        }
        held.visuals.insert(event.to_owned(), text.to_owned());
        true
    })
}

/// A registered visual alternative by event.
#[must_use]
pub fn get_visual_alternative(event: &str) -> Option<String> {
    with(|held| held.visuals.get(event).cloned())
}

/// Every event with a visual alternative, in event order.
#[must_use]
pub fn list_visual_alternatives() -> Vec<String> {
    with(|held| held.visuals.keys().cloned().collect())
}

/// Forgets a visual alternative. `false` when there was none.
pub fn unregister_visual_alternative(event: &str) -> bool {
    with(|held| held.visuals.remove(event).is_some())
}

/// The focus order a mod set, empty when it set none.
#[must_use]
pub fn get_focus_order() -> Vec<String> {
    with(|held| held.focus.clone())
}

/// Sets the focus order a screen reader walks. `true`, always.
pub fn set_focus_order(order: &[&str]) -> bool {
    with(|held| {
        held.focus = order.iter().map(|id| (*id).to_owned()).collect();
        true
    })
}

/// Queues a caption line, and answers how many are now queued.
pub fn show_caption(text: &str, duration: f64, source: &str) -> usize {
    with(|held| {
        held.captions.push(Caption {
            text: text.to_owned(),
            duration: duration.max(0.0),
            source: source.to_owned(),
            start: held.now,
        });
        held.captions.len()
    })
}

/// Moves the caption clock on by `seconds`, expiring each caption at exactly its duration,
/// and answers how many just expired.
pub fn advance_captions(seconds: f64) -> usize {
    with(|held| {
        held.now += seconds.max(0.0);
        let now = held.now;
        let before = held.captions.len();
        // expires at its duration, not a frame after: now - start < duration, never <=
        held.captions.retain(|caption| now - caption.start < caption.duration);
        before - held.captions.len()
    })
}

/// The captions showing at `moment`, in the order they were queued.
#[must_use]
pub fn caption_at(moment: f64) -> Vec<Caption> {
    with(|held| {
        held.captions
            .iter()
            .filter(|caption| moment >= caption.start && moment - caption.start < caption.duration)
            .cloned()
            .collect()
    })
}

/// How many captions are queued.
#[must_use]
pub fn caption_count() -> usize {
    with(|held| held.captions.len())
}

/// Drops every queued caption, and answers how many there were.
pub fn clear_captions() -> usize {
    with(|held| {
        let count = held.captions.len();
        held.captions.clear();
        count
    })
}

/// The WCAG relative luminance of an sRGB colour, `0.0` black to `1.0` white.
#[must_use]
pub fn relative_luminance(red: u8, green: u8, blue: u8) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.039_28 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

/// The WCAG contrast ratio of two sRGB colours, from `1.0` to `21.0`.
#[must_use]
pub fn contrast_ratio(first: (u8, u8, u8), second: (u8, u8, u8)) -> f64 {
    let a = relative_luminance(first.0, first.1, first.2);
    let b = relative_luminance(second.0, second.1, second.2);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

/// Whether two sRGB colours reach the [`READABLE_RATIO`] threshold.
#[must_use]
pub fn is_readable(first: (u8, u8, u8), second: (u8, u8, u8)) -> bool {
    contrast_ratio(first, second) >= READABLE_RATIO
}

/// Why a question about the client's own accessibility has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "key_repeat" => {
            "the client reads its own key-repeat delay from the operating system, so this host \
             cannot see how fast a held key repeats"
                .to_owned()
        }
        "input_assistance" => {
            "the client's input assistance changes how its own screens take input, a layer this \
             host does not reach"
                .to_owned()
        }
        "screen_reader" => {
            "the client speaks through its own screen reader, which this host cannot call, so a \
             mod's text cannot be announced here"
                .to_owned()
        }
        "options" => {
            "the client's accessibility options change how its own screens draw and read, a \
             layer this host does not reach"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The client's own accessibility this host cannot reach. Each one answers with its own sentence.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["key_repeat", "input_assistance", "screen_reader", "options"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refuse(what: &str) {
    crate::warn!(
        "a mod asked this host about the client's accessibility: {}",
        why_unknown(what)
    );
}

/// The client's key-repeat setting. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_key_repeat() -> Option<u32> {
    refuse("key_repeat");
    None
}

/// The client's input assistance. `None`: [`why_unknown`] says why.
#[must_use]
pub fn get_input_assistance() -> Option<String> {
    refuse("input_assistance");
    None
}

/// Asks the client's screen reader to speak `text`. `false`, always: [`why_unknown`] says why.
pub fn announce(_text: &str) -> bool {
    refuse("screen_reader");
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
            held.values.clear();
            held.watchers.clear();
            held.palettes.clear();
            held.widgets.clear();
            held.narrations.clear();
            held.visuals.clear();
            held.focus.clear();
            held.captions.clear();
            held.now = 0.0;
        });
        events::unsubscribe_all(EVENT_CHANGED);
    }

    #[test]
    fn the_option_set_reads_its_defaults_and_a_change_carries_old_to_new() {
        let _serial = serial();
        fresh();
        assert_eq!(get_settings(), AccessibilitySettings::default());
        assert!(!get_reduced_motion());
        assert_eq!(get_text_scale(), 1.0);
        assert_eq!(get_color_filters(), "");
        assert_eq!(options().len(), 7);

        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&heard);
        let watcher = watch(move |change| {
            into.lock()
                .unwrap()
                .push(format!("{} {}->{}", change.option, change.old.render(), change.new.render()));
        });

        let corrected = AccessibilitySettings {
            reduced_motion: true,
            text_scale: 1.5,
            ..AccessibilitySettings::default()
        };
        let changes = set_settings(&corrected).expect("every value fits");
        assert_eq!(changes.len(), 2, "only the two that moved");
        assert_eq!(changes[0].option, "reduced_motion");
        assert_eq!(changes[0].old, ConfigValue::Boolean(false));
        assert_eq!(changes[0].new, ConfigValue::Boolean(true));
        assert_eq!(changes[1].option, "text_scale");
        assert_eq!(changes[1].new, ConfigValue::Float(1.5));
        assert!(get_reduced_motion());
        assert_eq!(get_text_scale(), 1.5);
        assert_eq!(
            set_settings(&corrected).expect("every value fits"),
            Vec::new(),
            "setting the values they already hold fires nothing"
        );
        assert_eq!(
            *heard.lock().unwrap(),
            vec!["reduced_motion false->true".to_owned(), "text_scale 1->1.5".to_owned()]
        );
        assert!(events::is_registered_kind(EVENT_CHANGED));
        assert!(unwatch(watcher));
        assert!(!unwatch(watcher));
    }

    #[test]
    fn a_value_of_the_wrong_kind_or_off_the_bounds_is_refused() {
        let _serial = serial();
        fresh();
        assert_eq!(
            set_option("text_scale", ConfigValue::Text("big".to_owned())),
            Err(OptionError::WrongKind {
                option: "text_scale".to_owned(),
                wanted: "float".to_owned(),
                found: "text".to_owned(),
            })
        );
        assert_eq!(
            set_option("text_scale", ConfigValue::Float(9.0)),
            Err(OptionError::OutOfRange {
                option: "text_scale".to_owned(),
                min: 0.5,
                max: 4.0,
                got: 9.0,
            })
        );
        assert_eq!(
            set_option("nonsense", ConfigValue::Boolean(true)),
            Err(OptionError::Unknown {
                option: "nonsense".to_owned()
            })
        );
        assert_eq!(get_text_scale(), 1.0, "a refused value does not move");
        assert_eq!(
            get_option("text_scale").expect("declared").label,
            "Text size"
        );
    }

    #[test]
    fn a_palette_replaces_a_listed_colour_and_leaves_an_unlisted_one() {
        let _serial = serial();
        fresh();
        let palette = ColorPalette::new("hello:protanopia")
            .replace(TextColor::named("red"), TextColor::named("blue"))
            .replace(TextColor::hex(0x00ff00), TextColor::hex(0x0000ff));
        assert_eq!(palette.len(), 2);
        assert!(!palette.is_empty());
        assert_eq!(palette.lookup(&TextColor::named("red")), TextColor::named("blue"));
        assert_eq!(
            palette.lookup(&TextColor::named("green")),
            TextColor::named("green"),
            "a colour the palette does not list is returned unchanged"
        );
        let message = crate::chat::create_text("danger")
            .color(TextColor::named("red"))
            .then(crate::chat::create_text(" safe").color(TextColor::named("green")));
        let mapped = palette.apply(&message);
        assert_eq!(mapped.plain_text(), "danger safe");
        assert_eq!(mapped.spans()[0].style.color, Some(TextColor::named("blue")));
        assert_eq!(mapped.spans()[1].style.color, Some(TextColor::named("green")));
    }

    #[test]
    fn a_palette_registers_selects_and_lists() {
        let _serial = serial();
        fresh();
        assert!(register_color_alternative(ColorPalette::new("hello:deuteranopia")));
        assert!(
            !register_color_alternative(ColorPalette::new("hello:deuteranopia")),
            "a duplicate name is refused"
        );
        assert_eq!(list_color_alternatives(), vec!["hello:deuteranopia".to_owned()]);
        assert_eq!(
            get_color_alternative("hello:deuteranopia"),
            Some(ColorPalette::new("hello:deuteranopia"))
        );
        assert_eq!(list_color_alternatives().len(), 1);
        assert_eq!(
            select_color_alternative("hello:deuteranopia"),
            Ok(Some(OptionChange {
                option: PALETTE_OPTION.to_owned(),
                old: ConfigValue::Text(String::new()),
                new: ConfigValue::Text("hello:deuteranopia".to_owned()),
            }))
        );
        assert_eq!(get_color_filters(), "hello:deuteranopia");
        assert_eq!(
            select_color_alternative("nothing"),
            Err(OptionError::BadChoice {
                option: PALETTE_OPTION.to_owned(),
                choices: vec!["hello:deuteranopia".to_owned()],
                got: "nothing".to_owned(),
            })
        );
        assert_eq!(
            select_color_alternative(""),
            Ok(Some(OptionChange {
                option: PALETTE_OPTION.to_owned(),
                old: ConfigValue::Text("hello:deuteranopia".to_owned()),
                new: ConfigValue::Text(String::new()),
            }))
        );
    }

    #[test]
    fn a_widget_description_is_generated_per_kind_and_nothing_is_refused() {
        let _serial = serial();
        fresh();
        assert_eq!(
            get_widget_description("button", "Play"),
            Some("Play, button".to_owned())
        );
        assert_eq!(
            get_widget_description("slider", "Volume"),
            Some("Volume, slider".to_owned())
        );
        assert_eq!(get_widget_description("label", "Score"), Some("Score".to_owned()));
        assert_eq!(get_widget_description("", "Score"), Some("Score".to_owned()));
        assert_eq!(get_widget_description("button", ""), Some("button".to_owned()));
        assert_eq!(
            get_widget_description("  ", "  "),
            None,
            "there is nothing to describe"
        );
        assert!(register_accessible_widget(
            AccessibleWidget::new("hello:play", "button", "Play").described("Play the game")
        ));
        assert!(
            !register_accessible_widget(AccessibleWidget::new("hello:play", "button", "Play")),
            "a duplicate id is refused"
        );
        assert_eq!(list_accessible_widgets(), vec!["hello:play".to_owned()]);
        assert!(set_widget_description("hello:play", "Start"));
        assert!(!set_widget_description("nothing", "Start"));
        assert_eq!(
            get_accessible_widget("hello:play").expect("registered").description,
            Some("Start".to_owned())
        );
        assert!(unregister_accessible_widget("hello:play"));
        assert!(!unregister_accessible_widget("hello:play"));
    }

    #[test]
    fn captions_expire_at_exactly_their_duration_and_are_queryable() {
        let _serial = serial();
        fresh();
        assert_eq!(show_caption("Anvil falls", 2.0, "block.anvil.fall"), 1);
        assert_eq!(caption_count(), 1);
        assert_eq!(caption_at(0.5).len(), 1);
        assert_eq!(caption_at(0.5)[0].source, "block.anvil.fall");
        assert_eq!(advance_captions(1.5), 0, "not yet at its duration");
        assert_eq!(caption_count(), 1);
        assert_eq!(caption_at(1.5)[0].text, "Anvil falls");
        assert_eq!(advance_captions(0.5), 1, "expires exactly at 2.0");
        assert_eq!(caption_count(), 0);
        assert!(caption_at(2.0).is_empty());
        assert_eq!(show_caption("Ding", 1.0, "block.note"), 1);
        assert_eq!(advance_captions(3.0), 1);
        assert_eq!(clear_captions(), 0);
    }

    #[test]
    fn contrast_matches_the_wcag_formula_and_readability_uses_four_and_a_half() {
        assert_eq!(relative_luminance(0, 0, 0), 0.0);
        assert_eq!(relative_luminance(255, 255, 255), 1.0);
        assert_eq!(contrast_ratio((0, 0, 0), (255, 255, 255)), 21.0);
        assert_eq!(contrast_ratio((255, 255, 255), (0, 0, 0)), 21.0, "order does not matter");
        assert!(is_readable((0, 0, 0), (255, 255, 255)));
        assert!(!is_readable((119, 119, 119), (136, 136, 136)));
    }

    #[test]
    fn the_client_owned_options_and_screen_reader_are_refused() {
        let _serial = serial();
        fresh();
        assert_eq!(refusals().len(), 4);
        let reasons: std::collections::BTreeSet<String> =
            refusals().into_iter().map(|(_, why)| why).collect();
        assert_eq!(reasons.len(), 4, "every kind has its own sentence");
        assert!(why_unknown("key_repeat").contains("key-repeat"));
        assert_eq!(why_unknown("nonsense"), "nonsense is not something this host can see");
        assert!(get_key_repeat().is_none());
        assert!(get_input_assistance().is_none());
        assert!(!announce("hello"));
        assert!(!get_screen_reader_enabled());
    }

    #[test]
    fn narrations_visual_alternatives_and_focus_order_are_kept() {
        let _serial = serial();
        fresh();
        assert!(register_narration("hello:low_health", "Health is low"));
        assert!(!register_narration("hello:low_health", "other"));
        assert!(!register_narration("", "x"), "an empty id is refused");
        assert_eq!(get_narration("hello:low_health"), Some("Health is low".to_owned()));
        assert_eq!(list_narrations(), vec!["hello:low_health".to_owned()]);
        assert!(unregister_narration("hello:low_health"));

        assert!(register_visual_alternative("entity.creeper.primed", "Creeper hisses"));
        assert!(!register_visual_alternative("", "x"));
        assert_eq!(
            get_visual_alternative("entity.creeper.primed"),
            Some("Creeper hisses".to_owned())
        );
        assert_eq!(list_visual_alternatives(), vec!["entity.creeper.primed".to_owned()]);
        assert!(unregister_visual_alternative("entity.creeper.primed"));

        assert!(set_focus_order(&["hello:play", "hello:options"]));
        assert_eq!(
            get_focus_order(),
            vec!["hello:play".to_owned(), "hello:options".to_owned()]
        );
    }
}
