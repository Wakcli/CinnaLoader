//! Events: something happened, and the mods that care are told in a defined order.
//!
//! The bus belongs to the mod that uses it, and is driven by the loader's own phases: a
//! tick arrives because the loader ticked this mod, a render event because the client drew
//! a frame, `game_started` because `load` ran. Nothing here is fired by a timer or by a
//! thread of its own, so an event means what it says.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! events::subscribe(kind::SETTINGS_CHANGED, Priority::Normal, |event| {
//!     info!("a setting moved: {}", event.text().unwrap_or("?"));
//! });
//!
//! events::publish(Event::new("my_plugin:ready").with_text("warmed up"));
//! ```
//!
//! A listener that has to stop the rest does it the same way a cancellable event is
//! cancelled: by saying so, from inside the listener.
//!
//! ```
//! # use cinnaloader::prelude::*;
//! events::subscribe(kind::INPUT, Priority::High, |event| {
//!     if event.count() == Some(0x1b) {
//!         event.cancel();
//!         events::stop_propagation();
//!     }
//! });
//! ```

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// The kinds the loader fires itself. A mod may fire any name it likes; these are the ones
/// that arrive on their own.
pub mod kind {
    pub const GAME_STARTED: &str = "game_started";
    pub const GAME_STOPPING: &str = "game_stopping";
    pub const GAME_TICK_START: &str = "game_tick_start";
    pub const GAME_TICK_END: &str = "game_tick_end";
    pub const CLIENT_TICK: &str = "client_tick";
    pub const SERVER_TICK: &str = "server_tick";
    pub const RENDER_TICK: &str = "render_tick";
    pub const WORLD_TICK: &str = "world_tick";
    pub const RESOURCE_RELOAD: &str = "resource_reload";
    pub const SETTINGS_CHANGED: &str = "settings_changed";
    pub const LANGUAGE_CHANGED: &str = "language_changed";
    pub const SCREEN_CHANGED: &str = "screen_changed";
    pub const INPUT: &str = "input";
    pub const FOCUS_CHANGED: &str = "focus_changed";
    pub const WINDOW_RESIZED: &str = "window_resized";
    pub const GAME_PAUSED: &str = "game_paused";
    pub const GAME_RESUMED: &str = "game_resumed";
    pub const CRASH: &str = "crash";
    pub const MOD_LOADED: &str = "mod_loaded";
    pub const MOD_UNLOADED: &str = "mod_unloaded";
    pub const MOD_ENABLED: &str = "mod_enabled";
    pub const MOD_DISABLED: &str = "mod_disabled";
    pub const MOD_RELOADED: &str = "mod_reloaded";
    pub const COMMAND_REGISTERED: &str = "command_registered";
    pub const REGISTRY_CHANGED: &str = "registry_changed";
}

/// The kinds the loader can fire here. Anything else a mod fires is its own business.
pub const BUILT_IN_KINDS: [&str; 25] = [
    kind::GAME_STARTED,
    kind::GAME_STOPPING,
    kind::GAME_TICK_START,
    kind::GAME_TICK_END,
    kind::CLIENT_TICK,
    kind::SERVER_TICK,
    kind::RENDER_TICK,
    kind::WORLD_TICK,
    kind::RESOURCE_RELOAD,
    kind::SETTINGS_CHANGED,
    kind::LANGUAGE_CHANGED,
    kind::SCREEN_CHANGED,
    kind::INPUT,
    kind::FOCUS_CHANGED,
    kind::WINDOW_RESIZED,
    kind::GAME_PAUSED,
    kind::GAME_RESUMED,
    kind::CRASH,
    kind::MOD_LOADED,
    kind::MOD_UNLOADED,
    kind::MOD_ENABLED,
    kind::MOD_DISABLED,
    kind::MOD_RELOADED,
    kind::COMMAND_REGISTERED,
    kind::REGISTRY_CHANGED,
];

/// When a listener runs relative to the others of the same event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    Lowest,
    Low,
    Normal,
    High,
    Highest,
    /// Runs last of all, and is not allowed to change the event; for a mod that only
    /// watches what the others decided.
    Monitor,
}

impl Priority {
    /// Where the priority sits in a dispatch. `Monitor` is last by definition, not because
    /// it is somehow stronger, so it does not follow the enum's own order.
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            Self::Highest => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
            Self::Lowest => 4,
            Self::Monitor => 5,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lowest => "lowest",
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
            Self::Highest => "highest",
            Self::Monitor => "monitor",
        }
    }
}

impl fmt::Display for Priority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which leg of a dispatch a listener belongs to, coarser than priority: every `Pre`
/// listener runs before any `Main` one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DispatchPhase {
    Pre,
    Main,
    Post,
}

/// What an event carries, kept small on purpose: an event is a notification, not a
/// transport.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Payload {
    #[default]
    None,
    Flag(bool),
    Count(i64),
    Value(f64),
    Text(String),
    Pair(f64, f64),
}

/// One event.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    kind: String,
    payload: Payload,
    cancellable: bool,
    cancelled: bool,
    stopped: bool,
    /// Who sent it, so a listener can tell the loader's events from another mod's.
    source: String,
}

impl Event {
    /// An event of `kind`, carrying nothing.
    #[must_use]
    pub fn new(kind: &str) -> Self {
        Self {
            kind: kind.to_owned(),
            payload: Payload::None,
            cancellable: false,
            cancelled: false,
            stopped: false,
            source: crate::metadata::id(),
        }
    }

    /// The same, which a listener is allowed to cancel.
    #[must_use]
    pub fn cancellable(kind: &str) -> Self {
        Self {
            cancellable: true,
            ..Self::new(kind)
        }
    }

    #[must_use]
    pub fn with_flag(mut self, flag: bool) -> Self {
        self.payload = Payload::Flag(flag);
        self
    }

    #[must_use]
    pub fn with_count(mut self, count: i64) -> Self {
        self.payload = Payload::Count(count);
        self
    }

    #[must_use]
    pub fn with_value(mut self, value: f64) -> Self {
        self.payload = Payload::Value(value);
        self
    }

    #[must_use]
    pub fn with_text(mut self, text: &str) -> Self {
        self.payload = Payload::Text(text.to_owned());
        self
    }

    #[must_use]
    pub fn with_pair(mut self, left: f64, right: f64) -> Self {
        self.payload = Payload::Pair(left, right);
        self
    }

    /// Names the mod that sent it, for an event that is passed on.
    #[must_use]
    pub fn from(mut self, source: &str) -> Self {
        self.source = source.to_owned();
        self
    }

    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn payload(&self) -> &Payload {
        &self.payload
    }

    #[must_use]
    pub fn flag(&self) -> Option<bool> {
        match self.payload {
            Payload::Flag(flag) => Some(flag),
            _ => None,
        }
    }

    #[must_use]
    pub fn count(&self) -> Option<i64> {
        match self.payload {
            Payload::Count(count) => Some(count),
            _ => None,
        }
    }

    #[must_use]
    pub fn value(&self) -> Option<f64> {
        match self.payload {
            Payload::Value(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match &self.payload {
            Payload::Text(text) => Some(text),
            _ => None,
        }
    }

    #[must_use]
    pub fn pair(&self) -> Option<(f64, f64)> {
        match self.payload {
            Payload::Pair(left, right) => Some((left, right)),
            _ => None,
        }
    }

    /// Marks it cancelled, which is what the code that fired a cancellable event reads.
    pub fn cancel(&mut self) {
        if self.cancellable {
            self.cancelled = true;
        }
    }

    /// Un-marks it, for a later listener that disagrees.
    pub fn uncancel(&mut self) {
        self.cancelled = false;
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    #[must_use]
    pub fn is_cancellable(&self) -> bool {
        self.cancellable
    }

    /// Stops the listeners that have not run yet.
    pub fn stop(&mut self) {
        self.stopped = true;
    }

    /// Lets them run again.
    pub fn resume(&mut self) {
        self.stopped = false;
    }

    #[must_use]
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }
}

impl fmt::Display for Event {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} from {}", self.kind, self.source)
    }
}

/// What a listener is, for `get_listeners`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerInfo {
    pub id: u64,
    pub kind: String,
    pub priority: Priority,
    pub phase: DispatchPhase,
    pub once: bool,
    pub filtered: bool,
}

/// A listener's own gate: the events it was subscribed for, out of its kind.
type Filter = Box<dyn Fn(&Event) -> bool + Send>;

struct Listener {
    info: ListenerInfo,
    filter: Option<Filter>,
    run: Box<dyn FnMut(&mut Event) + Send>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn listeners() -> &'static Mutex<Vec<Listener>> {
    static LISTENERS: OnceLock<Mutex<Vec<Listener>>> = OnceLock::new();
    LISTENERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn pending() -> &'static Mutex<VecDeque<Event>> {
    static PENDING: OnceLock<Mutex<VecDeque<Event>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn render_queue() -> &'static Mutex<VecDeque<Event>> {
    static RENDER: OnceLock<Mutex<VecDeque<Event>>> = OnceLock::new();
    RENDER.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn kinds() -> &'static Mutex<Vec<String>> {
    static KINDS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    KINDS.get_or_init(|| Mutex::new(BUILT_IN_KINDS.iter().map(|kind| (*kind).to_owned()).collect()))
}

thread_local! {
    /// What the listener currently running has said about the event. Read back into the
    /// event once the listener returns, so a listener never holds a borrow of it.
    static CURRENT: RefCell<Current> = const { RefCell::new(Current { active: false, cancelled: false, stopped: false }) };
}

#[derive(Debug, Clone, Copy)]
struct Current {
    active: bool,
    cancelled: bool,
    stopped: bool,
}

/// Subscribes to `kind`, running on every matching event for as long as the mod is loaded.
/// Returns the listener's id, for `unsubscribe`.
pub fn subscribe<F>(kind: &str, priority: Priority, run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    add(kind, priority, DispatchPhase::Main, None, false, run)
}

/// Subscribes in a named dispatch phase, so a listener can promise to run before the rest.
pub fn subscribe_in<F>(
    kind: &str,
    phase: DispatchPhase,
    priority: Priority,
    run: F,
) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    add(kind, priority, phase, None, false, run)
}

/// Subscribes for one event only; the listener is removed before it runs again.
pub fn subscribe_once<F>(kind: &str, priority: Priority, run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    add(kind, priority, DispatchPhase::Main, None, true, run)
}

/// Subscribes to the events of `kind` that `filter` accepts.
pub fn subscribe_filtered<F, G>(kind: &str, priority: Priority, filter: G, run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
    G: Fn(&Event) -> bool + Send + 'static,
{
    add(kind, priority, DispatchPhase::Main, Some(Box::new(filter)), false, run)
}

/// The same as [`subscribe`], under the name the wishlist uses.
pub fn register_listener<F>(kind: &str, priority: Priority, run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    subscribe(kind, priority, run)
}

fn add(
    kind: &str,
    priority: Priority,
    phase: DispatchPhase,
    filter: Option<Filter>,
    once: bool,
    run: impl FnMut(&mut Event) + Send + 'static,
) -> u64 {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let info = ListenerInfo {
        id,
        kind: kind.to_owned(),
        priority,
        phase,
        once,
        filtered: filter.is_some(),
    };
    if let Ok(mut held) = listeners().lock() {
        held.push(Listener {
            info,
            filter,
            run: Box::new(run),
        });
    }
    id
}

/// Removes one listener.
pub fn unsubscribe(id: u64) -> bool {
    let mut held = match listeners().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    let before = held.len();
    held.retain(|listener| listener.info.id != id);
    held.len() != before
}

/// The same as [`unsubscribe`], under the name the wishlist uses.
pub fn unregister_listener(id: u64) -> bool {
    unsubscribe(id)
}

/// Removes every listener of `kind`, or every listener this mod has when `kind` is empty.
pub fn unsubscribe_all(kind: &str) -> usize {
    let mut held = match listeners().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    let before = held.len();
    held.retain(|listener| !kind.is_empty() && listener.info.kind != kind);
    before - held.len()
}

/// Every listener, in dispatch order.
#[must_use]
pub fn get_listeners() -> Vec<ListenerInfo> {
    let held = match listeners().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    let mut infos: Vec<ListenerInfo> = held.iter().map(|listener| listener.info.clone()).collect();
    infos.sort_by_key(|info| (info.kind.clone(), info.phase, info.priority, info.id));
    infos
}

/// How many listeners `kind` has, or all of them when `kind` is empty.
#[must_use]
pub fn get_listener_count(kind: &str) -> usize {
    let held = match listeners().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    match kind.is_empty() {
        true => held.len(),
        false => held.iter().filter(|listener| listener.info.kind == kind).count(),
    }
}

/// The order the listeners of `kind` will run in, which is what a mod debugging its
/// subscriptions needs.
#[must_use]
pub fn get_event_order(kind: &str) -> Vec<ListenerInfo> {
    get_listeners()
        .into_iter()
        .filter(|info| info.kind == kind)
        .collect()
}

/// Whether a kind is one the loader knows about, including the ones mods registered.
#[must_use]
pub fn is_registered_kind(kind: &str) -> bool {
    let held = match kinds().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    held.iter().any(|known| known == kind)
}

/// Every kind the loader knows about.
#[must_use]
pub fn known_kinds() -> Vec<String> {
    let held = match kinds().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    held.clone()
}

/// Teaches the bus a kind of its own, so `is_registered_kind` can tell a typo from a name.
pub fn register_event_type(kind: &str) -> bool {
    if kind.is_empty() || is_registered_kind(kind) {
        return false;
    }
    let mut held = match kinds().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    held.push(kind.to_owned());
    true
}

/// Forgets a kind, and every listener of it.
pub fn unregister_event_type(kind: &str) -> bool {
    let removed = unsubscribe_all(kind);
    let mut held = match kinds().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    let before = held.len();
    held.retain(|known| known != kind);
    held.len() != before || removed > 0
}

/// Moves a listener to another priority.
pub fn set_priority(id: u64, priority: Priority) -> bool {
    change(id, |info| info.priority = priority)
}

/// Moves a listener to another dispatch phase.
pub fn set_phase(id: u64, phase: DispatchPhase) -> bool {
    change(id, |info| info.phase = phase)
}

/// Replaces a listener's filter.
pub fn set_filter<G>(id: u64, filter: G) -> bool
where
    G: Fn(&Event) -> bool + Send + 'static,
{
    let mut held = match listeners().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    let Some(listener) = held.iter_mut().find(|listener| listener.info.id == id) else {
        return false;
    };
    listener.filter = Some(Box::new(filter));
    listener.info.filtered = true;
    true
}

fn change(id: u64, change: impl FnOnce(&mut ListenerInfo)) -> bool {
    let mut held = match listeners().lock() {
        Ok(held) => held,
        Err(error) => error.into_inner(),
    };
    match held.iter_mut().find(|listener| listener.info.id == id) {
        Some(listener) => {
            change(&mut listener.info);
            true
        }
        None => false,
    }
}

/// Runs `event` past its listeners now, and answers how many ran.
pub fn publish(mut event: Event) -> usize {
    dispatch(&mut event)
}

/// The same, for an event a listener is allowed to cancel.
pub fn post_cancellable(mut event: Event) -> usize {
    event.cancellable = true;
    dispatch(&mut event)
}

/// Queues `event` for the next dispatch point, which is the end of this tick.
pub fn post(event: Event) -> usize {
    let mut pending = match pending().lock() {
        Ok(pending) => pending,
        Err(error) => error.into_inner(),
    };
    pending.push_back(event);
    pending.len()
}

/// Runs everything queued, in the order it was queued.
pub fn dispatch_pending() -> usize {
    let queued: Vec<Event> = {
        let mut pending = match pending().lock() {
            Ok(pending) => pending,
            Err(error) => error.into_inner(),
        };
        pending.drain(..).collect()
    };
    let count = queued.len();
    for mut event in queued {
        dispatch(&mut event);
    }
    count
}

/// Drops everything queued without running it.
pub fn clear_pending() -> usize {
    let mut pending = match pending().lock() {
        Ok(pending) => pending,
        Err(error) => error.into_inner(),
    };
    let count = pending.len();
    pending.clear();
    count
}

/// How many events are waiting for the next dispatch point.
#[must_use]
pub fn pending_count() -> usize {
    pending()
        .lock()
        .map(|pending| pending.len())
        .unwrap_or_default()
}

/// Queues everything waiting for the render phase instead, for an event that must be seen
/// while the client is drawing.
pub fn dispatch_render_thread() -> usize {
    let queued: Vec<Event> = {
        let mut pending = match pending().lock() {
            Ok(pending) => pending,
            Err(error) => error.into_inner(),
        };
        pending.drain(..).collect()
    };
    let count = queued.len();
    let mut render = match render_queue().lock() {
        Ok(render) => render,
        Err(error) => error.into_inner(),
    };
    render.extend(queued);
    count
}

/// Runs what was queued for the render phase. The loader calls this at the start of a
/// frame.
pub fn dispatch_render_queue() -> usize {
    let queued: Vec<Event> = {
        let mut render = match render_queue().lock() {
            Ok(render) => render,
            Err(error) => error.into_inner(),
        };
        render.drain(..).collect()
    };
    let count = queued.len();
    for mut event in queued {
        dispatch(&mut event);
    }
    count
}

/// Runs the queue on a thread of its own. Returns immediately; a listener that must see the
/// client's state should not be subscribed this way.
pub fn dispatch_async() -> usize {
    let count = pending_count();
    std::thread::spawn(dispatch_pending);
    count
}

/// Runs the queue now, on the thread that called it, which for a mod is the thread the
/// loader ticks it on.
pub fn dispatch_main_thread() -> usize {
    dispatch_pending()
}

/// Whether a listener is running right now, on this thread.
#[must_use]
pub fn is_dispatching() -> bool {
    CURRENT.with(|current| current.borrow().active)
}

/// Whether the event being dispatched has been cancelled so far.
#[must_use]
pub fn is_cancelled() -> bool {
    CURRENT.with(|current| current.borrow().cancelled)
}

/// Cancels the event being dispatched, from inside a listener.
pub fn cancel_event() {
    CURRENT.with(|current| current.borrow_mut().cancelled = true);
}

/// Stops the listeners that have not run yet.
pub fn stop_propagation() {
    CURRENT.with(|current| current.borrow_mut().stopped = true);
}

/// Lets them run again.
pub fn resume_propagation() {
    CURRENT.with(|current| current.borrow_mut().stopped = false);
}

/// Whether propagation has been stopped so far.
#[must_use]
pub fn is_stopped() -> bool {
    CURRENT.with(|current| current.borrow().stopped)
}

fn dispatch(event: &mut Event) -> usize {
    let (mut matching, rest) = {
        let mut held = match listeners().lock() {
            Ok(held) => held,
            Err(error) => error.into_inner(),
        };
        let taken: Vec<Listener> = held.drain(..).collect();
        let mut matching = Vec::new();
        let mut rest = Vec::new();
        for listener in taken {
            if listener.info.kind == event.kind {
                matching.push(listener);
            } else {
                rest.push(listener);
            }
        }
        (matching, rest)
    };
    matching.sort_by_key(|listener| {
        (
            listener.info.phase,
            listener.info.priority.rank(),
            listener.info.id,
        )
    });

    let mut ran = 0;
    let mut keep = rest;
    for mut listener in matching {
        if event.stopped {
            keep.push(listener);
            continue;
        }
        let passes = listener
            .filter
            .as_ref()
            .is_none_or(|filter| filter(event));
        if !passes {
            keep.push(listener);
            continue;
        }
        begin_scope(event.cancelled);
        (listener.run)(event);
        let (cancelled, stopped) = end_scope();
        if cancelled {
            event.cancel();
        }
        if stopped {
            event.stopped = true;
        }
        ran += 1;
        if !listener.info.once {
            keep.push(listener);
        }
    }

    if let Ok(mut held) = listeners().lock() {
        held.extend(keep);
    }
    ran
}

fn begin_scope(cancelled: bool) {
    CURRENT.with(|current| {
        *current.borrow_mut() = Current {
            active: true,
            cancelled,
            stopped: false,
        };
    });
}

fn end_scope() -> (bool, bool) {
    CURRENT.with(|current| {
        let state = *current.borrow();
        *current.borrow_mut() = Current {
            active: false,
            cancelled: false,
            stopped: false,
        };
        (state.cancelled, state.stopped)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn listeners_run_in_phase_then_priority_order() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kind = "order-probe";
        register_event_type(kind);
        for (phase, priority, label) in [
            (DispatchPhase::Main, Priority::Low, "main-low"),
            (DispatchPhase::Pre, Priority::Lowest, "pre"),
            (DispatchPhase::Main, Priority::Highest, "main-high"),
            (DispatchPhase::Post, Priority::Normal, "post"),
            (DispatchPhase::Main, Priority::Monitor, "monitor"),
        ] {
            let seen = Arc::clone(&seen);
            subscribe_in(kind, phase, priority, move |_| {
                seen.lock().unwrap().push(label);
            });
        }

        assert_eq!(publish(Event::new(kind)), 5);
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["pre", "main-high", "main-low", "monitor", "post"]
        );
        unsubscribe_all(kind);
    }

    #[test]
    fn a_cancellable_event_can_be_cancelled_and_stopped() {
        let kind = "cancel-probe";
        subscribe(kind, Priority::High, |event| {
            event.cancel();
            event.stop();
        });
        let second = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&second);
        subscribe(kind, Priority::Normal, move |_| {
            *flag.lock().unwrap() = true;
        });

        let mut event = Event::cancellable(kind);
        let ran = dispatch(&mut event);
        assert!(event.is_cancelled());
        assert!(event.is_stopped());
        assert_eq!(ran, 1, "the lower-priority listener must not run");
        assert!(!*second.lock().unwrap(), "and must not have been called");
        assert!(!Event::new(kind).is_cancellable());
        unsubscribe_all(kind);
    }

    #[test]
    fn a_filter_keeps_a_listener_out_of_events_it_does_not_want() {
        let kind = "filter-probe";
        let ran = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&ran);
        subscribe_filtered(
            kind,
            Priority::Normal,
            |event| event.count() == Some(2),
            move |event| seen.lock().unwrap().push(event.count().unwrap()),
        );
        publish(Event::new(kind).with_count(1));
        publish(Event::new(kind).with_count(2));
        assert_eq!(*ran.lock().unwrap(), vec![2]);
        unsubscribe_all(kind);
    }

    #[test]
    fn a_once_listener_runs_once() {
        let kind = "once-probe";
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        subscribe_once(kind, Priority::Normal, move |_| {
            *counter.lock().unwrap() += 1;
        });
        publish(Event::new(kind));
        publish(Event::new(kind));
        assert_eq!(*calls.lock().unwrap(), 1);
        assert_eq!(get_listener_count(kind), 0);
    }

    #[test]
    fn what_is_posted_waits_for_the_next_dispatch_point() {
        let kind = "posted-probe";
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        subscribe(kind, Priority::Normal, move |_| {
            *counter.lock().unwrap() += 1;
        });
        clear_pending();
        post(Event::new(kind));
        assert!(pending_count() >= 1, "posting queues rather than dispatch");
        assert_eq!(*calls.lock().unwrap(), 0, "posting does not dispatch");
        dispatch_pending();
        assert_eq!(pending_count(), 0);
        assert_eq!(*calls.lock().unwrap(), 1);
        unsubscribe_all(kind);
    }

    #[test]
    fn a_listener_can_be_moved_and_removed() {
        let kind = "move-probe";
        let id = subscribe(kind, Priority::Low, |_| {});
        assert_eq!(get_listener_count(kind), 1);
        assert!(set_priority(id, Priority::Highest));
        assert!(set_phase(id, DispatchPhase::Post));
        let moved = get_event_order(kind);
        assert_eq!(moved[0].priority, Priority::Highest);
        assert_eq!(moved[0].phase, DispatchPhase::Post);
        assert!(unsubscribe(id));
        assert_eq!(get_listener_count(kind), 0);
        assert!(!unsubscribe(id));
    }

    #[test]
    fn a_kind_can_be_registered_and_forgotten() {
        assert!(is_registered_kind(kind::CLIENT_TICK));
        assert!(!is_registered_kind("mine:probe"));
        assert!(register_event_type("mine:probe"));
        assert!(is_registered_kind("mine:probe"));
        assert!(known_kinds().len() > BUILT_IN_KINDS.len());
        assert!(unregister_event_type("mine:probe"));
        assert!(!is_registered_kind("mine:probe"));
    }

    #[test]
    fn a_listener_running_on_the_bus_sees_its_own_state() {
        let kind = "state-probe";
        let inside = Arc::new(Mutex::new((false, false, false)));
        let seen = Arc::clone(&inside);
        subscribe(kind, Priority::Normal, move |_| {
            let mut state = seen.lock().unwrap();
            *state = (is_dispatching(), is_cancelled(), is_stopped());
        });
        publish(Event::cancellable(kind));
        assert_eq!(*inside.lock().unwrap(), (true, false, false));
        assert!(!is_dispatching(), "the scope ends with the dispatch");
        unsubscribe_all(kind);
    }
}
