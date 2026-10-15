//! Notifications and toast messages: the model a mod owns, and the client's toast it cannot draw.
//!
//! A [`Notification`] is real data — an id, a title, a body, a [`Level`], a channel, a
//! caller-supplied timestamp, an optional [`Action`], a duration, a [`Position`] and an icon.
//! [`NotificationQueue`] orders by **level first (error, warning, success, info), then
//! timestamp, then id**, and answers push, pop, peek, dismiss and clear with a real count.
//! Pushing the same id twice **replaces** the first entry and reports it as [`Push::Replaced`]
//! instead of adding a second. A channel is enabled by default; a disabled one **drops** its
//! notifications and [`dropped_count`] counts them. [`History`] is a ring with a cap that drops
//! the oldest entry one over it; [`unread_count`] falls as notifications are dismissed. A
//! notification renders as text with [`render`](Notification::render) and [`Display`].
//!
//! What this host cannot do is put any of it on the client's screen. A toast is the client's,
//! a click on it is the client's input, and its notification centre is the client's; each is
//! refused with a reason in [`why_unknown`], listed by [`refusals`], and never faked. The one
//! place a mod host can put text is the loader's overlay line (section 29).
//!
//! ```
//! use cinnaloader::notifications::{self, Level, Notification, Push};
//!
//! let shown = notifications::show(
//!     Notification::new("hello:boot", "Loaded", "112 mods")
//!         .level(Level::Success)
//!         .channel("hello:core")
//!         .at(10),
//! );
//! assert_eq!(shown, Push::Added);
//! assert_eq!(notifications::get_active().len(), 1);
//! assert_eq!(notifications::unread_count(), 1);
//! assert!(notifications::dismiss("hello:boot"));
//! assert_eq!(notifications::unread_count(), 0);
//! assert!(!notifications::show_toast(&Notification::new("x", "x", "x")));
//! ```

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// How many notifications the mod's history keeps by default.
pub const DEFAULT_HISTORY_CAP: usize = 64;
/// How long a notification shows, in seconds, unless a caller says otherwise.
pub const DEFAULT_DURATION: f64 = 5.0;

/// How urgent a notification is. The queue puts the most severe first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum Level {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl Level {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    /// How severe, higher first: error 3, warning 2, success 1, info 0.
    #[must_use]
    pub fn severity(self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Success => 1,
            Self::Warning => 2,
            Self::Error => 3,
        }
    }
}

/// Where a notification would be placed, when a client had somewhere to place it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Position {
    TopLeft,
    #[default]
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Position {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TopLeft => "top_left",
            Self::TopRight => "top_right",
            Self::BottomLeft => "bottom_left",
            Self::BottomRight => "bottom_right",
        }
    }
}

/// What a notification's action would do, when the client dispatched the click to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub label: String,
    pub command: String,
}

impl Action {
    #[must_use]
    pub fn new(label: &str, command: &str) -> Self {
        Self {
            label: label.to_owned(),
            command: command.to_owned(),
        }
    }
}

/// One thing a mod wants to tell the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub level: Level,
    pub channel: String,
    /// The time the caller says it happened, which orders notifications of one level.
    pub timestamp: u64,
    pub action: Option<Action>,
    pub duration: f64,
    pub position: Position,
    pub icon: String,
}

impl Notification {
    /// A notification with the defaults a caller would expect: info, the default channel,
    /// timestamp zero, a five-second duration and no action.
    #[must_use]
    pub fn new(id: &str, title: &str, body: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            level: Level::Info,
            channel: "minecraft:default".to_owned(),
            timestamp: 0,
            action: None,
            duration: DEFAULT_DURATION,
            position: Position::TopRight,
            icon: String::new(),
        }
    }

    #[must_use]
    pub fn level(mut self, level: Level) -> Self {
        self.level = level;
        self
    }

    #[must_use]
    pub fn channel(mut self, channel: &str) -> Self {
        self.channel = channel.to_owned();
        self
    }

    /// Sets the caller-supplied timestamp.
    #[must_use]
    pub fn at(mut self, timestamp: u64) -> Self {
        self.timestamp = timestamp;
        self
    }

    #[must_use]
    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration = clamp_duration(seconds);
        self
    }

    #[must_use]
    pub fn position(mut self, position: Position) -> Self {
        self.position = position;
        self
    }

    #[must_use]
    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = icon.to_owned();
        self
    }

    #[must_use]
    pub fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    /// The notification as one line of text.
    #[must_use]
    pub fn render(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for Notification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "[{}] {}: {} ({})",
            self.level.as_str(),
            self.title,
            self.body,
            self.channel
        )?;
        if let Some(action) = &self.action {
            write!(formatter, " -> {}", action.label)?;
        }
        Ok(())
    }
}

fn clamp_duration(seconds: f64) -> f64 {
    if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        DEFAULT_DURATION
    }
}

/// What a push did. A duplicate id reports [`Push::Replaced`]; a disabled channel reports
/// [`Push::Dropped`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum Push {
    Added,
    Replaced,
    Dropped,
}

fn rank(a: &Notification, b: &Notification) -> std::cmp::Ordering {
    b.level
        .severity()
        .cmp(&a.level.severity())
        .then(a.timestamp.cmp(&b.timestamp))
        .then(a.id.cmp(&b.id))
}

/// A mod-owned queue of notifications, ordered level first, then time, then id.
#[derive(Debug, Clone, Default)]
pub struct NotificationQueue {
    entries: Vec<Notification>,
}

impl NotificationQueue {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a notification, or replaces the one with that id.
    pub fn push(&mut self, notification: Notification) -> Push {
        if let Some(existing) = self.entries.iter_mut().find(|held| held.id == notification.id) {
            *existing = notification;
            self.entries.sort_by(rank);
            return Push::Replaced;
        }
        self.entries.push(notification);
        self.entries.sort_by(rank);
        Push::Added
    }

    /// Takes the most urgent notification off the queue.
    pub fn pop(&mut self) -> Option<Notification> {
        if self.entries.is_empty() {
            None
        } else {
            Some(self.entries.remove(0))
        }
    }

    /// The most urgent notification, without taking it.
    #[must_use]
    pub fn peek(&self) -> Option<Notification> {
        self.entries.first().cloned()
    }

    /// Removes a notification by id.
    pub fn dismiss(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|held| held.id != id);
        self.entries.len() != before
    }

    /// Changes a notification in place, keeping the order.
    pub fn update(&mut self, id: &str, change: impl FnOnce(&mut Notification)) -> bool {
        match self.entries.iter_mut().find(|held| held.id == id) {
            Some(entry) => {
                change(entry);
                self.entries.sort_by(rank);
                true
            }
            None => false,
        }
    }

    /// A notification by id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Notification> {
        self.entries.iter().find(|held| held.id == id).cloned()
    }

    /// Every notification, in queue order.
    #[must_use]
    pub fn ordered(&self) -> Vec<Notification> {
        self.entries.clone()
    }

    /// Drops everything, answering how many went.
    pub fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A bounded history a mod owns. A push one past the cap drops the oldest entry.
#[derive(Debug, Clone)]
pub struct History {
    capacity: usize,
    entries: VecDeque<Notification>,
    unread: BTreeSet<String>,
}

impl History {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
            unread: BTreeSet::new(),
        }
    }

    /// Records a notification, answering with the one it dropped when it was full. A repeat of
    /// an id it already holds replaces it in place.
    pub fn push(&mut self, notification: Notification) -> Option<Notification> {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|held| held.id == notification.id)
        {
            let id = notification.id.clone();
            *existing = notification;
            self.unread.insert(id);
            return None;
        }
        let dropped = if self.entries.len() >= self.capacity {
            let oldest = self.entries.pop_front();
            if let Some(oldest) = &oldest {
                self.unread.remove(&oldest.id);
            }
            oldest
        } else {
            None
        };
        self.unread.insert(notification.id.clone());
        self.entries.push_back(notification);
        dropped
    }

    /// Marks a notification read, answering whether it was still unread.
    pub fn mark_read(&mut self, id: &str) -> bool {
        self.unread.remove(id)
    }

    /// How many recorded notifications have not been dismissed.
    #[must_use]
    pub fn unread_count(&self) -> usize {
        self.unread.len()
    }

    /// A notification by position, oldest first.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<Notification> {
        self.entries.get(index).cloned()
    }

    /// Every notification, oldest first.
    #[must_use]
    pub fn get_all(&self) -> Vec<Notification> {
        self.entries.iter().cloned().collect()
    }

    /// Notifications whose title or body contains `needle`.
    #[must_use]
    pub fn find(&self, needle: &str) -> Vec<Notification> {
        self.entries
            .iter()
            .filter(|held| held.title.contains(needle) || held.body.contains(needle))
            .cloned()
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Drops everything, answering how many went.
    pub fn clear(&mut self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        self.unread.clear();
        count
    }
}

/// A mod's notification preferences.
#[derive(Debug, Clone, PartialEq)]
pub struct Preferences {
    /// Whether the mod accepts notifications at all.
    pub enabled: bool,
    pub position: Position,
    pub default_duration: f64,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            position: Position::TopRight,
            default_duration: DEFAULT_DURATION,
        }
    }
}

type Handler = Box<dyn FnMut(&Notification) + Send>;

fn queue() -> &'static Mutex<NotificationQueue> {
    static QUEUE: OnceLock<Mutex<NotificationQueue>> = OnceLock::new();
    QUEUE.get_or_init(|| Mutex::new(NotificationQueue::new()))
}

fn with_queue<T>(body: impl FnOnce(&mut NotificationQueue) -> T) -> T {
    let mut guard = queue().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn history() -> &'static Mutex<History> {
    static HISTORY: OnceLock<Mutex<History>> = OnceLock::new();
    HISTORY.get_or_init(|| Mutex::new(History::new(DEFAULT_HISTORY_CAP)))
}

fn with_history<T>(body: impl FnOnce(&mut History) -> T) -> T {
    let mut guard = history().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn channels() -> &'static Mutex<BTreeMap<String, bool>> {
    static CHANNELS: OnceLock<Mutex<BTreeMap<String, bool>>> = OnceLock::new();
    CHANNELS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn handlers() -> &'static Mutex<BTreeMap<String, Vec<Handler>>> {
    static HANDLERS: OnceLock<Mutex<BTreeMap<String, Vec<Handler>>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn prefs() -> &'static Mutex<Preferences> {
    static PREFS: OnceLock<Mutex<Preferences>> = OnceLock::new();
    PREFS.get_or_init(|| Mutex::new(Preferences::default()))
}

static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Shows a notification: it enters the mod's queue and its history unless its channel, or the
/// mod's notifications, are off.
pub fn show(notification: Notification) -> Push {
    let enabled = prefs()
        .lock()
        .map(|held| held.enabled)
        .unwrap_or(true);
    if !enabled || !is_channel_enabled(&notification.channel) {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return Push::Dropped;
    }
    let outcome = with_queue(|queue| queue.push(notification.clone()));
    with_history(|history| history.push(notification));
    outcome
}

/// Shows an info notification.
pub fn show_info(id: &str, title: &str, body: &str) -> Push {
    show(Notification::new(id, title, body).level(Level::Info))
}

/// Shows a success notification.
pub fn show_success(id: &str, title: &str, body: &str) -> Push {
    show(Notification::new(id, title, body).level(Level::Success))
}

/// Shows a warning notification.
pub fn show_warning(id: &str, title: &str, body: &str) -> Push {
    show(Notification::new(id, title, body).level(Level::Warning))
}

/// Shows an error notification.
pub fn show_error(id: &str, title: &str, body: &str) -> Push {
    show(Notification::new(id, title, body).level(Level::Error))
}

fn dispatch(event: &str, notification: &Notification) -> usize {
    let taken = {
        let mut guard = handlers().lock().unwrap_or_else(|error| error.into_inner());
        guard.remove(event).unwrap_or_default()
    };
    let count = taken.len();
    let mut returned = taken;
    for handler in returned.iter_mut() {
        handler(notification);
    }
    handlers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(event.to_owned(), returned);
    count
}

/// Takes a notification off the queue, marks it read, and answers whether it was there.
pub fn dismiss(id: &str) -> bool {
    let (notification, removed) = with_queue(|queue| (queue.get(id), queue.dismiss(id)));
    with_history(|history| history.mark_read(id));
    if let Some(notification) = notification {
        dispatch("dismiss", &notification);
    }
    removed
}

/// Takes every notification off the queue, marks them read, and answers how many went.
pub fn dismiss_all() -> usize {
    let all = with_queue(|queue| {
        let all = queue.ordered();
        queue.clear();
        all
    });
    for notification in &all {
        with_history(|history| history.mark_read(&notification.id));
        dispatch("dismiss", notification);
    }
    all.len()
}

/// Every notification in the queue, in queue order.
#[must_use]
pub fn get_active() -> Vec<Notification> {
    with_queue(|queue| queue.ordered())
}

/// Every notification in the mod's history, oldest first.
#[must_use]
pub fn get_history() -> Vec<Notification> {
    with_history(|history| history.get_all())
}

/// How many recorded notifications have not been dismissed.
#[must_use]
pub fn unread_count() -> usize {
    with_history(|history| history.unread_count())
}

/// How many notifications the mod's history holds.
#[must_use]
pub fn history_count() -> usize {
    with_history(|history| history.len())
}

/// Drops the mod's history, answering how many went.
pub fn clear_history() -> usize {
    with_history(|history| history.clear())
}

/// How many notifications have been dropped by a disabled channel or by preferences being off.
#[must_use]
pub fn dropped_count() -> u64 {
    DROPPED.load(Ordering::Relaxed)
}

/// Sets how long a queued notification shows.
pub fn set_duration(id: &str, seconds: f64) -> bool {
    with_queue(|queue| queue.update(id, |entry| entry.duration = clamp_duration(seconds)))
}

/// Sets where a queued notification would be placed.
pub fn set_position(id: &str, position: Position) -> bool {
    with_queue(|queue| queue.update(id, |entry| entry.position = position))
}

/// Sets a queued notification's icon.
pub fn set_icon(id: &str, icon: &str) -> bool {
    with_queue(|queue| queue.update(id, |entry| entry.icon = icon.to_owned()))
}

/// Sets a queued notification's action.
pub fn set_action(id: &str, action: Action) -> bool {
    with_queue(|queue| queue.update(id, |entry| entry.action = Some(action)))
}

/// Registers a handler for an event, answering how many that event now has. `dismiss` fires
/// when the mod dismisses a notification; `click` is recorded and never called.
pub fn register_handler(event: &str, handler: impl FnMut(&Notification) + Send + 'static) -> usize {
    let mut handlers = handlers().lock().unwrap_or_else(|error| error.into_inner());
    let list = handlers.entry(event.to_owned()).or_default();
    list.push(Box::new(handler));
    list.len()
}

/// Registers a handler for a dismissal, which the mod's own `dismiss` fires.
pub fn on_dismiss(handler: impl FnMut(&Notification) + Send + 'static) -> usize {
    register_handler("dismiss", handler)
}

/// Registers a handler for a click on a notification. A click is the client's input, so
/// nothing here calls it; it is recorded only.
pub fn on_click(handler: impl FnMut(&Notification) + Send + 'static) -> usize {
    register_handler("click", handler)
}

/// Forgets every handler for an event, answering how many went.
pub fn clear_handlers(event: &str) -> usize {
    handlers()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(event)
        .map(|list| list.len())
        .unwrap_or(0)
}

/// The mod's notification preferences.
#[must_use]
pub fn get_preferences() -> Preferences {
    prefs()
        .lock()
        .map(|held| held.clone())
        .unwrap_or_default()
}

/// Sets the mod's notification preferences, answering what took.
pub fn set_preferences(preferences: Preferences) -> Preferences {
    let taken = Preferences {
        enabled: preferences.enabled,
        position: preferences.position,
        default_duration: clamp_duration(preferences.default_duration),
    };
    if let Ok(mut held) = prefs().lock() {
        *held = taken.clone();
    }
    taken
}

/// Registers a channel, answering whether it is new.
pub fn register_channel(name: &str) -> bool {
    channels()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(name.to_owned(), true)
        .is_none()
}

/// Turns a channel on or off, answering its previous state when it was known.
pub fn set_channel_enabled(name: &str, enabled: bool) -> Option<bool> {
    channels()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(name.to_owned(), enabled)
}

/// Whether a channel accepts notifications. An unknown channel is on.
#[must_use]
pub fn is_channel_enabled(name: &str) -> bool {
    channels()
        .lock()
        .map(|channels| channels.get(name).copied().unwrap_or(true))
        .unwrap_or(true)
}

/// How many channels are registered.
#[must_use]
pub fn channel_count() -> usize {
    channels().lock().map(|channels| channels.len()).unwrap_or(0)
}

/// Shows a notification on the client's screen. `false`: a toast is the client's.
pub fn show_toast(_notification: &Notification) -> bool {
    crate::warn!(
        "a mod asked this host to show a notification on the client: {}",
        why_unknown("toast")
    );
    false
}

/// Why a notification question about the client has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "toast" => {
            "drawing a notification on the client's screen is the client's; the loader's overlay \
             line (section 29) is the one place a mod host can put text"
                .to_owned()
        }
        "click" => {
            "a notification's action is dispatched by the client's UI when the player clicks it, \
             and nothing here receives a click"
                .to_owned()
        }
        "center" => {
            "the client's notification centre, its own toasts and its history are the client's; a \
             mod keeps its own queue and ring"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["toast", "click", "center"]
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

    fn fresh() {
        with_queue(|queue| *queue = NotificationQueue::new());
        with_history(|history| *history = History::new(DEFAULT_HISTORY_CAP));
        channels()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        handlers()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        if let Ok(mut held) = prefs().lock() {
            *held = Preferences::default();
        }
        DROPPED.store(0, Ordering::Relaxed);
    }

    #[test]
    fn the_queue_orders_by_level_then_time_then_id() {
        let mut queue = NotificationQueue::new();
        let _ = queue.push(Notification::new("queue:info", "i", "b").at(100));
        let _ = queue.push(Notification::new("queue:err-late", "e", "b").level(Level::Error).at(300));
        let _ = queue.push(Notification::new("queue:err-early", "e", "b").level(Level::Error).at(200));
        let _ = queue.push(Notification::new("queue:warn", "w", "b").level(Level::Warning).at(50));
        let _ = queue.push(Notification::new("queue:warn-z", "w", "b").level(Level::Warning).at(50));
        let order: Vec<String> = queue.ordered().into_iter().map(|held| held.id).collect();
        assert_eq!(
            order,
            vec![
                "queue:err-early".to_owned(),
                "queue:err-late".to_owned(),
                "queue:warn".to_owned(),
                "queue:warn-z".to_owned(),
                "queue:info".to_owned(),
            ],
            "error before warning before info, then time, then id"
        );
        assert_eq!(queue.len(), 5);
        assert_eq!(queue.peek().map(|held| held.id), Some("queue:err-early".to_owned()));
    }

    #[test]
    fn a_duplicate_id_replaces_and_reports() {
        let _serial = serial();
        fresh();
        let mut queue = NotificationQueue::new();
        assert_eq!(queue.push(Notification::new("dup", "first", "v1")), Push::Added);
        assert_eq!(
            queue.push(Notification::new("dup", "first", "v2")),
            Push::Replaced,
            "the same id replaces the first"
        );
        assert_eq!(queue.len(), 1, "and does not add a second");
        assert_eq!(queue.get("dup").map(|held| held.body), Some("v2".to_owned()));

        assert_eq!(show_info("hello:dup", "t", "a"), Push::Added);
        assert_eq!(show_info("hello:dup", "t", "b"), Push::Replaced);
        assert_eq!(get_active().len(), 1);
        assert_eq!(get_active()[0].body, "b");
        assert_eq!(get_history().len(), 1);
    }

    #[test]
    fn pop_peek_dismiss_and_clear_answer_the_queue() {
        let _serial = serial();
        fresh();
        let _ = show_error("q:err", "e", "b");
        let _ = show_warning("q:warn", "w", "b");
        assert_eq!(get_active().len(), 2);
        let mut queue = NotificationQueue::new();
        let _ = queue.push(Notification::new("a", "a", "b").level(Level::Warning));
        let _ = queue.push(Notification::new("b", "b", "b").level(Level::Error));
        assert_eq!(queue.pop().map(|held| held.id), Some("b".to_owned()));
        assert_eq!(queue.len(), 1);
        assert!(queue.dismiss("a"));
        assert!(!queue.dismiss("a"), "already gone");
        assert!(queue.is_empty());
        assert_eq!(queue.clear(), 0);

        assert!(dismiss("q:err"));
        assert!(!dismiss("q:err"));
        assert_eq!(get_active().len(), 1);
        assert_eq!(dismiss_all(), 1);
        assert!(get_active().is_empty());
        assert_eq!(dismiss_all(), 0);
    }

    #[test]
    fn a_disabled_channel_drops_and_is_counted() {
        let _serial = serial();
        fresh();
        assert!(register_channel("hello:muted"));
        assert!(!register_channel("hello:muted"));
        assert_eq!(channel_count(), 1);
        assert!(is_channel_enabled("hello:muted"), "on by default");
        assert_eq!(set_channel_enabled("hello:muted", false), Some(true));
        assert!(!is_channel_enabled("hello:muted"));
        let dropped = show(Notification::new("m1", "t", "b").channel("hello:muted"));
        assert_eq!(dropped, Push::Dropped);
        assert_eq!(dropped_count(), 1);
        assert!(get_active().is_empty());
        assert!(get_history().is_empty());
        assert_eq!(set_channel_enabled("hello:muted", true), Some(false));
        assert_eq!(
            show(Notification::new("m2", "t", "b").channel("hello:muted")),
            Push::Added
        );
        assert_eq!(dropped_count(), 1, "an enabled channel drops nothing");
    }

    #[test]
    fn the_history_is_a_ring_that_drops_exactly_the_oldest() {
        let _serial = serial();
        fresh();
        let mut history = History::new(3);
        assert_eq!(history.capacity(), 3);
        for id in ["h0", "h1", "h2"] {
            assert_eq!(history.push(Notification::new(id, "t", "b")), None);
        }
        assert_eq!(history.len(), 3);
        assert_eq!(history.unread_count(), 3);
        let dropped = history
            .push(Notification::new("h3", "t", "b"))
            .expect("one over the cap drops the oldest");
        assert_eq!(dropped.id, "h0", "exactly the oldest");
        assert_eq!(history.len(), 3);
        assert_eq!(history.get(0).map(|held| held.id), Some("h1".to_owned()));
        assert_eq!(history.find("t").len(), 3);
        assert_eq!(history.clear(), 3);
    }

    #[test]
    fn an_unread_count_falls_when_a_notification_is_dismissed() {
        let _serial = serial();
        fresh();
        let _ = show_info("u1", "t", "b");
        let _ = show_info("u2", "t", "b");
        let _ = show_success("u3", "t", "b");
        assert_eq!(unread_count(), 3);
        assert_eq!(history_count(), 3);
        assert!(dismiss("u2"));
        assert_eq!(unread_count(), 2, "a dismiss reduces the unread count");
        assert!(!dismiss("u2"));
        assert_eq!(unread_count(), 2);
        assert_eq!(dismiss_all(), 2);
        assert_eq!(unread_count(), 0);
        assert_eq!(clear_history(), 3);
    }

    #[test]
    fn a_notification_renders_as_text() {
        let _serial = serial();
        fresh();
        let plain = Notification::new("hello:boot", "Loaded", "112 mods");
        assert_eq!(plain.render(), "[info] Loaded: 112 mods (minecraft:default)");
        assert_eq!(plain.to_string(), plain.render());
        let with_action = plain
            .clone()
            .level(Level::Error)
            .channel("hello:core")
            .action(Action::new("Retry", "/hello retry"));
        assert_eq!(
            with_action.render(),
            "[error] Loaded: 112 mods (hello:core) -> Retry"
        );
    }

    #[test]
    fn a_dismiss_handler_is_called_and_a_click_handler_only_recorded() {
        let _serial = serial();
        fresh();
        let dismissed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let into = std::sync::Arc::clone(&dismissed);
        let clicks = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let clicker = std::sync::Arc::clone(&clicks);
        assert_eq!(
            on_dismiss(move |notification: &Notification| {
                into.lock().unwrap().push(notification.id.clone());
            }),
            1
        );
        assert_eq!(
            on_click(move |_: &Notification| {
                clicker.fetch_add(1, Ordering::SeqCst);
            }),
            1
        );
        let _ = show_warning("hello:fire", "t", "b");
        assert!(dismiss("hello:fire"));
        assert_eq!(dismissed.lock().unwrap().as_slice(), ["hello:fire"]);
        assert_eq!(
            clicks.load(Ordering::SeqCst),
            0,
            "nothing here receives a click"
        );
        assert_eq!(clear_handlers("dismiss"), 1);
        assert_eq!(clear_handlers("click"), 1);
        assert_eq!(clear_handlers("dismiss"), 0);
    }

    #[test]
    fn the_clients_ui_is_refused_with_a_reason_each() {
        let _serial = serial();
        fresh();
        let notification = Notification::new("hello:t", "t", "b");
        assert!(!show_toast(&notification));
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("toast").contains("overlay"));
        assert!(why_unknown("click").contains("client's UI"));
        assert!(why_unknown("center").contains("notification centre"));

        let taken = set_preferences(Preferences {
            enabled: false,
            position: Position::BottomLeft,
            default_duration: 2.5,
        });
        assert!(!taken.enabled);
        assert_eq!(taken.position, Position::BottomLeft);
        assert_eq!(get_preferences().default_duration, 2.5);
        assert_eq!(show_info("off", "t", "b"), Push::Dropped);
        assert_eq!(dropped_count(), 1);
    }
}
