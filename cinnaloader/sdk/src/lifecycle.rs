//! When things run: the phases the loader drives, and the hooks a mod answers.
//!
//! The loader walks a fixed order. It calls the mod's own entry points, which the SDK turns
//! into the trait methods in [`Mod`](crate::Mod):
//!
//! | phase | when | what is safe |
//! | --- | --- | --- |
//! | `Load` | the mod's `load`, once the host table is installed | declaring settings and the manifest |
//! | `Initialize` | after every mod has loaded | reading other mods' settings through the loader |
//! | `PostInitialize` | right after `Initialize` | final wiring, before the first tick |
//! | `Ready` | after every mod has been post-initialised | everything the mod will ever have |
//! | `TickStart` / `Tick` / `TickEnd` | each loader tick, about 60/s | per-tick work |
//! | `RenderStart` / `RenderEnd` | each client frame, in-client only | nothing blocking: this runs on the frame thread |
//! | `Unload` | the loader drops the mod (the CLI path) | releasing what it holds |
//! | `Shutdown` | after `Unload`, when the process is going away | the last chance to flush |
//! | `Error` | the loader caught a panic from this mod | logging; the mod is then switched off |
//!
//! Phases that need the game — world load, player join, suspend and resume — are not here
//! yet: they need hooks into the client, and a phase that never fires would be a lie.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use crate::modinfo::ModVersion;

/// A point in the loader's order at which a mod can act.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Before any mod loads. Nothing is callable yet; only [`defer_until_ready`] and friends
    /// are useful here.
    PreLoad,
    /// The mod's `load`.
    Load,
    /// After every mod has loaded.
    Initialize,
    /// Immediately after [`Phase::Initialize`].
    PostInitialize,
    /// Every mod is loaded and initialised.
    Ready,
    /// The start of one tick.
    TickStart,
    /// The tick itself.
    Tick,
    /// The end of one tick.
    TickEnd,
    /// The client started a frame. In-client only.
    RenderStart,
    /// The client finished a frame. In-client only.
    RenderEnd,
    /// The mod is being dropped.
    Unload,
    /// The process is going away.
    Shutdown,
    /// The loader caught a panic from this mod.
    Error,
}

impl Phase {
    /// The value the loader passes across the ABI.
    #[must_use]
    pub const fn code(self) -> u32 {
        match self {
            Self::PreLoad => 0,
            Self::Load => 1,
            Self::Initialize => 2,
            Self::PostInitialize => 3,
            Self::Ready => 4,
            Self::TickStart => 5,
            Self::Tick => 6,
            Self::TickEnd => 7,
            Self::RenderStart => 8,
            Self::RenderEnd => 9,
            Self::Unload => 10,
            Self::Shutdown => 11,
            Self::Error => 12,
        }
    }

    /// The phase a code names, or `None` when it is not one this SDK knows.
    #[must_use]
    pub const fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::PreLoad,
            1 => Self::Load,
            2 => Self::Initialize,
            3 => Self::PostInitialize,
            4 => Self::Ready,
            5 => Self::TickStart,
            6 => Self::Tick,
            7 => Self::TickEnd,
            8 => Self::RenderStart,
            9 => Self::RenderEnd,
            10 => Self::Unload,
            11 => Self::Shutdown,
            12 => Self::Error,
            _ => return None,
        })
    }

    /// The name a log line uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PreLoad => "pre_load",
            Self::Load => "load",
            Self::Initialize => "initialize",
            Self::PostInitialize => "post_initialize",
            Self::Ready => "ready",
            Self::TickStart => "tick_start",
            Self::Tick => "tick",
            Self::TickEnd => "tick_end",
            Self::RenderStart => "render_start",
            Self::RenderEnd => "render_end",
            Self::Unload => "unload",
            Self::Shutdown => "shutdown",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

const PHASE_NONE: u32 = u32::MAX;

static PHASE: AtomicU32 = AtomicU32::new(PHASE_NONE);
static INITIALISED: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);
static CLEANUPS: Mutex<Vec<fn()>> = Mutex::new(Vec::new());
static SHUTDOWNS: Mutex<Vec<fn()>> = Mutex::new(Vec::new());
static DEFERRED: Mutex<Vec<fn()>> = Mutex::new(Vec::new());

/// The phase being dispatched right now, or [`Phase::PreLoad`] before the loader starts.
#[must_use]
pub fn current_phase() -> Phase {
    let code = PHASE.load(Ordering::SeqCst);
    if code == PHASE_NONE {
        return Phase::PreLoad;
    }
    Phase::from_code(code).unwrap_or(Phase::PreLoad)
}

/// Whether every mod has loaded and this mod has been initialised.
#[must_use]
pub fn is_initialized() -> bool {
    INITIALISED.load(Ordering::SeqCst)
}

/// Whether the loader reached [`Phase::Ready`].
#[must_use]
pub fn is_ready() -> bool {
    READY.load(Ordering::SeqCst)
}

/// Registers a function to run as this mod unloads. Runs after the mod's own
/// [`Mod::unload`](crate::Mod::unload), and before its `shutdown`.
pub fn register_cleanup(cleanup: fn()) {
    if let Ok(mut cleanups) = CLEANUPS.lock() {
        cleanups.push(cleanup);
    }
}

/// Registers a function to run when the process is going away, after cleanups.
pub fn register_shutdown_handler(handler: fn()) {
    if let Ok(mut handlers) = SHUTDOWNS.lock() {
        handlers.push(handler);
    }
}

/// Queues a function to run once the loader is ready, which is the first point every other
/// mod is loaded. Runs before the first tick.
pub fn defer_until_ready(deferred: fn()) {
    if READY.load(Ordering::SeqCst) {
        deferred();
        return;
    }
    if let Ok(mut queue) = DEFERRED.lock() {
        queue.push(deferred);
    }
}

/// Drops everything queued by [`defer_until_ready`] that has not run yet.
pub fn cancel_pending_hooks() {
    if let Ok(mut queue) = DEFERRED.lock() {
        queue.clear();
    }
}

/// How much is still queued, for a test or a diagnostic.
#[must_use]
pub fn pending_hooks() -> usize {
    DEFERRED.lock().map(|queue| queue.len()).unwrap_or(0)
}

/// Dispatches one phase, and marks where the mod is.
pub(crate) fn enter(phase: Phase) {
    PHASE.store(phase.code(), Ordering::SeqCst);
    match phase {
        Phase::Initialize | Phase::PostInitialize => {
            INITIALISED.store(true, Ordering::SeqCst);
        }
        Phase::Ready => {
            INITIALISED.store(true, Ordering::SeqCst);
            READY.store(true, Ordering::SeqCst);
            if let Ok(mut queue) = DEFERRED.lock() {
                for deferred in queue.drain(..) {
                    deferred();
                }
            }
        }
        _ => {}
    }
    announce(phase);
}

/// Fires the events that go with a phase, and delivers whatever a mod queued. This is the
/// only place the bus is driven from, so an event always means the phase really happened.
fn announce(phase: Phase) {
    use crate::events::{self, kind, Event};
    let event = match phase {
        Phase::Initialize => Event::new(kind::MOD_LOADED).with_text(&crate::metadata::id()),
        Phase::TickStart => Event::new(kind::CLIENT_TICK),
        Phase::TickEnd => Event::new(kind::GAME_TICK_END),
        Phase::RenderStart => Event::new(kind::RENDER_TICK),
        Phase::RenderEnd => Event::new(kind::RENDER_TICK).with_flag(false),
        Phase::Unload => Event::new(kind::MOD_UNLOADED).with_text(&crate::metadata::id()),
        Phase::Shutdown => Event::new(kind::GAME_STOPPING).with_text(&crate::metadata::id()),
        Phase::Error => Event::new(kind::CRASH),
        _ => return,
    };
    events::publish(event);
    if matches!(phase, Phase::TickStart) {
        // A tick is both the game's tick and the client's: this loader runs inside the
        // client, so it fires both names for the same moment.
        events::publish(Event::new(kind::GAME_TICK_START));
    }
    if matches!(phase, Phase::RenderStart) {
        events::dispatch_render_queue();
    }
    if matches!(phase, Phase::TickStart | Phase::TickEnd) {
        events::dispatch_pending();
    }
}

/// Runs everything [`register_cleanup`] queued. The loader calls this after the mod's own
/// `unload`, so a cleanup can count on teardown having happened.
pub(crate) fn run_cleanups() {
    if let Ok(mut cleanups) = CLEANUPS.lock() {
        for cleanup in cleanups.drain(..) {
            cleanup();
        }
    }
}

/// Runs everything [`register_shutdown_handler`] queued, after the mod's `shutdown`.
pub(crate) fn run_shutdown_handlers() {
    if let Ok(mut handlers) = SHUTDOWNS.lock() {
        for handler in handlers.drain(..) {
            handler();
        }
    }
}

/// What the mod's `migrate` needs to describe where a config came from.
///
/// The loader stores the version a mod last ran as `__version` in its config file, so a mod
/// that has been updated can move its own data across.
#[must_use]
pub fn previous_version() -> Option<ModVersion> {
    crate::config("__version").map(|version| ModVersion::parse(&version))
}

/// Records the version this run is, so the next start can tell whether a migration is due.
pub(crate) fn record_version(version: &ModVersion) {
    crate::set_config("__version", &version.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static RAN: AtomicUsize = AtomicUsize::new(0);

    fn bump() {
        RAN.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn phases_round_trip_through_their_codes() {
        for phase in [
            Phase::PreLoad,
            Phase::Load,
            Phase::Initialize,
            Phase::PostInitialize,
            Phase::Ready,
            Phase::TickStart,
            Phase::Tick,
            Phase::TickEnd,
            Phase::RenderStart,
            Phase::RenderEnd,
            Phase::Unload,
            Phase::Shutdown,
            Phase::Error,
        ] {
            assert_eq!(Phase::from_code(phase.code()), Some(phase));
            assert!(!phase.name().is_empty());
        }
        assert_eq!(Phase::from_code(999), None);
    }

    #[test]
    fn deferred_hooks_run_once_the_loader_is_ready() {
        cancel_pending_hooks();
        RAN.store(0, Ordering::SeqCst);
        defer_until_ready(bump);
        defer_until_ready(bump);
        assert_eq!(pending_hooks(), 2);
        enter(Phase::Ready);
        assert_eq!(RAN.load(Ordering::SeqCst), 2);
        assert_eq!(pending_hooks(), 0);
        assert!(is_ready() && is_initialized());
        // A hook deferred after ready runs immediately.
        defer_until_ready(bump);
        assert_eq!(RAN.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn cleanups_and_shutdown_handlers_run_when_the_loader_calls_them() {
        RAN.store(0, Ordering::SeqCst);
        register_cleanup(bump);
        register_shutdown_handler(bump);
        run_cleanups();
        assert_eq!(RAN.load(Ordering::SeqCst), 1);
        run_shutdown_handlers();
        assert_eq!(RAN.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_phase_is_readable_while_it_is_dispatching() {
        enter(Phase::Tick);
        assert_eq!(current_phase(), Phase::Tick);
        enter(Phase::RenderStart);
        assert_eq!(current_phase(), Phase::RenderStart);
    }
}
