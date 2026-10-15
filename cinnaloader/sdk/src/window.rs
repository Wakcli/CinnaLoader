//! The window the client draws into: the facts a launcher hands over, and the geometry that
//! follows from them.
//!
//! This host does not own the client's window. It reads the facts a launcher provides — the
//! size the crate already knows from [`crate::game::get_window`], a monitor list a caller can
//! set, a cursor position — and does real arithmetic on them: a logical↔physical DPI
//! conversion that round trips, a resize model that fires only when the size moves, cursor
//! clamping to the window box, an aspect ratio, and a primary monitor with its flag. Every
//! request that would need the client's own window handle — its title, its focus, fullscreen,
//! a cursor grab, attention, minimise, maximise, restore, close, its icon, its border, its
//! resizability — is refused with its own sentence in [`why_unknown`], and [`refusals`]
//! lists them.
//!
//! ```
//! use cinnaloader::window;
//!
//! window::set_size(1280, 720);
//! assert_eq!(window::get_size(), Some((1280, 720)));
//! assert_eq!(window::get_aspect_ratio(), Some(1280.0 / 720.0));
//! assert_eq!(window::set_cursor(5000.0, -3.0), (1280.0, 0.0));
//! assert!(!window::set_fullscreen(true), "the client owns its window");
//! ```

use std::sync::{Mutex, OnceLock};

use crate::events::{self, kind as event_kind, Event, Priority};

/// The event a size change fires on, and the one a close request would fire on.
pub const CLOSE_REQUESTED: &str = "window_close_requested";

/// The window, as geometry: its pixel size and the scale factor a caller reads it at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
}

/// One monitor, as the launcher describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
    pub refresh_rate: u32,
    pub primary: bool,
}

#[derive(Debug, Clone, Default)]
struct State {
    size: Option<(u32, u32)>,
    position: Option<(i32, i32)>,
    monitors: Vec<Monitor>,
    cursor: (f32, f32),
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn with<T>(body: impl FnOnce(&mut State) -> T) -> T {
    let mut guard = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// The window as one value, or `None` when neither the launcher nor the host facts name a
/// size.
#[must_use]
pub fn get() -> Option<Window> {
    get_size().map(|(width, height)| Window {
        width,
        height,
        scale_factor: get_scale_factor(),
    })
}

/// The window's handle. `None`: see [`why_unknown`].
#[must_use]
pub fn get_id() -> Option<u64> {
    None
}

/// The window's title. `None`: see [`why_unknown`].
#[must_use]
pub fn get_title() -> Option<String> {
    None
}

/// Asks the client to set its title. Refused: the client owns its window.
pub fn set_title(_title: &str) -> bool {
    refuse("set its title", "title")
}

/// How big the window is, in physical pixels.
#[must_use]
pub fn get_size() -> Option<(u32, u32)> {
    with(|state| state.size).or_else(|| {
        crate::game::get_window().map(|window| (window.width, window.height))
    })
}

/// Records a size the launcher reports, and fires [`on_resized`] only when it changed.
pub fn set_size(width: u32, height: u32) -> bool {
    let changed = with(|state| {
        let previous = state.size;
        state.size = Some((width, height));
        previous != Some((width, height))
    });
    if changed {
        let _ = events::publish(
            Event::new(event_kind::WINDOW_RESIZED)
                .with_pair(f64::from(width), f64::from(height)),
        );
    }
    changed
}

/// Where the window's top-left corner sits. `None` until a launcher reports one.
#[must_use]
pub fn get_position() -> Option<(i32, i32)> {
    with(|state| state.position)
}

/// Records a position the launcher reports, answering whether it moved.
pub fn set_position(x: i32, y: i32) -> bool {
    with(|state| {
        let previous = state.position;
        state.position = Some((x, y));
        previous != Some((x, y))
    })
}

/// The display resolution: the primary monitor's, or the window's when no monitor is known.
#[must_use]
pub fn get_resolution() -> Option<(u32, u32)> {
    get_monitor()
        .map(|monitor| (monitor.width, monitor.height))
        .or_else(get_size)
}

/// Records the display mode a launcher reports on the primary monitor, answering whether it
/// changed. Changing the real mode is the operating system's business, not this host's.
pub fn set_resolution(width: u32, height: u32) -> bool {
    with(|state| match state.monitors.iter().position(|monitor| monitor.primary) {
        Some(index) => {
            let held = &mut state.monitors[index];
            let changed = (held.width, held.height) != (width, height);
            held.width = width;
            held.height = height;
            changed
        }
        None => {
            state.monitors.push(Monitor {
                name: "primary".to_owned(),
                width,
                height,
                scale_factor: 1.0,
                refresh_rate: 0,
                primary: true,
            });
            true
        }
    })
}

/// The scale factor: the primary monitor's, or the host window fact, or 1.
#[must_use]
pub fn get_scale_factor() -> f32 {
    if let Some(monitor) = get_monitor() {
        return monitor.scale_factor;
    }
    crate::game::get_window()
        .map(|window| window.scale)
        .unwrap_or(1.0)
}

/// Whether the window has focus. `false`: see [`why_unknown`].
#[must_use]
pub fn is_focused() -> bool {
    false
}

/// Whether the window is minimised. `false`: see [`why_unknown`].
#[must_use]
pub fn is_minimized() -> bool {
    false
}

/// Whether the window is maximised. `false`: see [`why_unknown`].
#[must_use]
pub fn is_maximized() -> bool {
    false
}

/// Whether the window is fullscreen. `false`: see [`why_unknown`].
#[must_use]
pub fn is_fullscreen() -> bool {
    false
}

/// Asks the client to go fullscreen. Refused: the client owns its window.
pub fn set_fullscreen(_fullscreen: bool) -> bool {
    refuse("go fullscreen", "fullscreen")
}

/// Asks the client to drop its border. Refused: the client owns its window.
pub fn set_borderless(_borderless: bool) -> bool {
    refuse("drop its border", "borderless")
}

/// Asks the client to change its resizability. Refused: the client owns its window.
pub fn set_resizable(_resizable: bool) -> bool {
    refuse("change its resizability", "resizable")
}

/// Asks the client to set its icon. Refused: the client owns its window.
pub fn set_icon(_path: &str) -> bool {
    refuse("set its icon", "icon")
}

/// The primary monitor, or the first one known.
#[must_use]
pub fn get_monitor() -> Option<Monitor> {
    with(|state| {
        state
            .monitors
            .iter()
            .find(|monitor| monitor.primary)
            .or_else(|| state.monitors.first())
            .cloned()
    })
}

/// Every monitor the launcher reported.
#[must_use]
pub fn get_monitors() -> Vec<Monitor> {
    with(|state| state.monitors.clone())
}

/// Replaces the monitor list, answering how many it holds.
pub fn set_monitors(monitors: Vec<Monitor>) -> usize {
    with(|state| {
        state.monitors = monitors;
        state.monitors.len()
    })
}

/// The primary monitor's refresh rate, when it reports a nonzero one.
#[must_use]
pub fn get_refresh_rate() -> Option<u32> {
    get_monitor()
        .map(|monitor| monitor.refresh_rate)
        .filter(|rate| *rate > 0)
}

/// Places the pointer, clamped to the window box, and answers where it landed.
#[must_use]
pub fn set_cursor(x: f32, y: f32) -> (f32, f32) {
    let (width, height) = get_size().unwrap_or((0, 0));
    let placed = (x.clamp(0.0, width as f32), y.clamp(0.0, height as f32));
    with(|state| state.cursor = placed);
    placed
}

/// Where the pointer was last placed.
#[must_use]
pub fn get_cursor() -> (f32, f32) {
    with(|state| state.cursor)
}

/// The window's width over its height, when the height is known and nonzero.
#[must_use]
pub fn get_aspect_ratio() -> Option<f32> {
    get_size()
        .filter(|(_, height)| *height > 0)
        .map(|(width, height)| width as f32 / height as f32)
}

/// A logical pixel count in physical pixels. Rounds half away from zero, which is what
/// [`f32::round`] does, so the same logical size always lands on the same physical pixel.
#[must_use]
pub fn to_physical(logical: f32, scale: f32) -> u32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (logical * scale).round().max(0.0) as u32
}

/// A physical pixel count in logical pixels, which is the inverse of [`to_physical`] up to
/// half a physical pixel divided by the scale.
#[must_use]
pub fn to_logical(physical: u32, scale: f32) -> f32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    physical as f32 / scale
}

/// Asks the client to grab the cursor. Refused: the client owns its window.
pub fn set_cursor_grabbed(_grabbed: bool) -> bool {
    refuse("grab the cursor", "cursor_grab")
}

/// Asks the client to flash for attention. Refused: the client owns its window.
pub fn request_attention() -> bool {
    refuse("ask for attention", "attention")
}

/// Asks the client to minimise. Refused: the client owns its window.
pub fn minimize() -> bool {
    refuse("minimise", "minimize")
}

/// Asks the client to maximise. Refused: the client owns its window.
pub fn maximize() -> bool {
    refuse("maximise", "maximize")
}

/// Asks the client to restore. Refused: the client owns its window.
pub fn restore() -> bool {
    refuse("restore", "restore")
}

/// Asks the client to close. Refused: the client owns its window.
pub fn close() -> bool {
    refuse("close", "close")
}

/// Runs when the size changes, on the §6 bus. Answers the listener id.
pub fn on_resized<F>(run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::subscribe(event_kind::WINDOW_RESIZED, Priority::Normal, run)
}

/// Runs when focus changes, on the §6 bus. This host never fires it: focus is the client's.
pub fn on_focus_changed<F>(run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::subscribe(event_kind::FOCUS_CHANGED, Priority::Normal, run)
}

/// Runs when the client asks to close, on the §6 bus. This host never fires it either.
pub fn on_close_requested<F>(run: F) -> u64
where
    F: FnMut(&mut Event) + Send + 'static,
{
    events::register_event_type(CLOSE_REQUESTED);
    events::subscribe(CLOSE_REQUESTED, Priority::Normal, run)
}

fn refuse(action: &str, what: &str) -> bool {
    crate::warn!("a mod asked the window to {action}: {}", why_unknown(what));
    false
}

/// Why a window question has no answer in this host.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "id" => "the client owns its window handle and exposes no id to this host".to_owned(),
        "title" => "the client owns its window title; this host can neither read nor set it".to_owned(),
        "focus" => "whether the window has focus is the client's state, behind its native handle".to_owned(),
        "minimized" => "whether the window is minimised is the client's own window state".to_owned(),
        "maximized" => "whether the window is maximised is the client's own window state".to_owned(),
        "fullscreen" => "whether the window is fullscreen is the client's own window state".to_owned(),
        "borderless" | "resizable" | "icon" => {
            format!("the window's own {what} is the client's, behind its native handle")
        }
        "cursor_grab" => "grabbing the cursor needs the client's window handle".to_owned(),
        "attention" => "flashing for attention needs the client's window handle".to_owned(),
        "minimize" | "maximize" | "restore" | "close" => {
            format!("the client's window decides when to {what}; this host has no handle to do it")
        }
        _ => format!("{what} is not something this host can see"),
    }
}

/// The window questions this host cannot answer, each with its reason.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "id",
        "title",
        "focus",
        "minimized",
        "maximized",
        "fullscreen",
        "borderless",
        "resizable",
        "icon",
        "cursor_grab",
        "attention",
        "minimize",
        "maximize",
        "restore",
        "close",
    ]
    .iter()
    .map(|what| ((*what).to_owned(), why_unknown(what)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        with(|state| *state = State::default());
    }

    #[test]
    fn size_position_and_aspect_come_from_the_facts_and_are_settable() {
        let _serial = serial();
        fresh();
        assert!(set_size(1280, 720));
        assert_eq!(get_size(), Some((1280, 720)));
        assert_eq!(get_aspect_ratio(), Some(1280.0 / 720.0));
        assert_eq!(
            get(),
            Some(Window {
                width: 1280,
                height: 720,
                scale_factor: 1.0,
            })
        );
        assert!(set_position(10, 20));
        assert_eq!(get_position(), Some((10, 20)));
        assert!(!set_position(10, 20), "the same position is not a move");
        assert!(!set_size(1280, 720), "the same size is not a change");
    }

    #[test]
    fn a_resize_fires_once_and_only_on_a_change() {
        let _serial = serial();
        fresh();
        assert!(set_size(640, 480));
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        let id = on_resized(move |event| {
            assert_eq!(event.pair(), Some((1024.0, 768.0)));
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(set_size(1024, 768));
        assert!(!set_size(1024, 768), "the same size fires nothing");
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert!(events::unsubscribe(id));
    }

    #[test]
    fn the_cursor_clamps_to_the_window_box() {
        let _serial = serial();
        fresh();
        set_size(800, 600);
        assert_eq!(set_cursor(5000.0, -3.0), (800.0, 0.0));
        assert_eq!(get_cursor(), (800.0, 0.0));
        assert_eq!(set_cursor(400.0, 300.0), (400.0, 300.0));
        assert_eq!(set_cursor(-1.0, 9000.0), (0.0, 600.0));
    }

    #[test]
    fn dpi_conversion_round_trips_and_rounds_half_away_from_zero() {
        assert_eq!(to_physical(120.0, 2.0), 240);
        assert_eq!(to_logical(240, 2.0), 120.0);
        assert_eq!(to_physical(1.1, 1.5), 2, "1.65 rounds to 2");
        assert!((to_logical(2, 1.5) - 4.0 / 3.0).abs() < 1e-6);
        assert_eq!(to_physical(10.0, 0.0), 10, "a zero scale is taken as one");
        assert_eq!(to_physical(-5.0, 2.0), 0);
    }

    #[test]
    fn monitors_are_settable_and_the_primary_is_choosable() {
        let _serial = serial();
        fresh();
        let monitors = vec![
            Monitor {
                name: "left".to_owned(),
                width: 1280,
                height: 720,
                scale_factor: 1.0,
                refresh_rate: 60,
                primary: false,
            },
            Monitor {
                name: "main".to_owned(),
                width: 2560,
                height: 1440,
                scale_factor: 2.0,
                refresh_rate: 144,
                primary: true,
            },
        ];
        assert_eq!(set_monitors(monitors), 2);
        assert_eq!(get_monitors().len(), 2);
        let primary = get_monitor().expect("a primary was set");
        assert_eq!(primary.name, "main");
        assert!(primary.primary);
        assert_eq!(get_scale_factor(), 2.0);
        assert_eq!(get_refresh_rate(), Some(144));
        assert_eq!(get_resolution(), Some((2560, 1440)));
        assert!(set_resolution(1920, 1080));
        assert_eq!(get_resolution(), Some((1920, 1080)));
        assert!(!set_resolution(1920, 1080));
    }

    #[test]
    fn a_request_the_client_owns_is_refused_with_its_sentence() {
        let _serial = serial();
        fresh();
        assert!(!set_title("hello"));
        assert!(!set_fullscreen(true));
        assert!(!set_borderless(true));
        assert!(!set_resizable(false));
        assert!(!set_icon("hello.png"));
        assert!(!set_cursor_grabbed(true));
        assert!(!request_attention());
        assert!(!minimize());
        assert!(!maximize());
        assert!(!restore());
        assert!(!close());
        assert_eq!(refusals().len(), 15);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_queries_that_need_the_handle_answer_false_or_none() {
        assert!(!is_focused());
        assert!(!is_minimized());
        assert!(!is_maximized());
        assert!(!is_fullscreen());
        assert!(get_id().is_none());
        assert!(get_title().is_none());
    }
}
