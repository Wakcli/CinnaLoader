//! What the game is doing, as far as a mod host inside the client can honestly tell.
//!
//! Two kinds of answer live here, and they are kept apart on purpose:
//!
//! * what this loader **observes itself** — that it is running, for how long, how many
//!   frames and ticks have gone by, the frames and ticks per second, the game version the
//!   client's own assets name, the address the client was launched with, the window, the
//!   phase, and the two managers it does own (its resource stack and its mod registry);
//! * what it **cannot reach**: the client's own screen, world, server, player, dimension,
//!   game mode, network manager and renderer are objects inside the client that this host has
//!   no handle on. Those answer `None` with the reason rather than a plausible-looking
//!   value, and the same for the three requests the loader cannot make.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! if let (Some(fps), Some(tps)) = (game::current_fps(), game::current_tps()) {
//!     info!("{fps:.0} fps, {tps:.0} ticks/s, phase {}", game::current_phase());
//! }
//! assert!(game::is_running());
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Counters the loader keeps as the client runs. The SDK's frame body feeds this.
#[derive(Debug, Default)]
struct Counters {
    frames: AtomicU64,
    ticks: AtomicU64,
    /// The last window of frames, for a rate that settles rather than jumping.
    window_frames: AtomicU32,
    window_started: Mutex<Option<Instant>>,
    /// The rate as of the last window close.
    fps: Mutex<Option<f64>>,
    tps: Mutex<Option<f64>>,
    last_frame: Mutex<Option<Instant>>,
    started: OnceLock<Instant>,
    slowest_frame_millis: AtomicU64,
}

fn counters() -> &'static Counters {
    static COUNTERS: OnceLock<Counters> = OnceLock::new();
    COUNTERS.get_or_init(Counters::default)
}

fn started() -> &'static Instant {
    counters().started.get_or_init(Instant::now)
}

/// How many frames one rate window covers.
const RATE_WINDOW: u32 = 30;

/// One client frame. The SDK's frame body calls this, so the numbers below are about the
/// client's real loop rather than a timer of ours.
pub fn note_frame(dt_millis: u32) {
    let counters = counters();
    counters.frames.fetch_add(1, Ordering::Relaxed);
    counters.ticks.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut last) = counters.last_frame.lock() {
        *last = Some(Instant::now());
    }
    let slowest = counters.slowest_frame_millis.load(Ordering::Relaxed);
    if u64::from(dt_millis) > slowest {
        counters
            .slowest_frame_millis
            .store(u64::from(dt_millis), Ordering::Relaxed);
    }
    let frames = counters.window_frames.fetch_add(1, Ordering::Relaxed) + 1;
    let mut close = false;
    if let Ok(mut window) = counters.window_started.lock() {
        match *window {
            None => *window = Some(Instant::now()),
            Some(begin) if frames >= RATE_WINDOW => {
                let seconds = begin.elapsed().as_secs_f64();
                if seconds > 0.0 {
                    if let Ok(mut fps) = counters.fps.lock() {
                        *fps = Some(f64::from(frames) / seconds);
                    }
                    if let Ok(mut tps) = counters.tps.lock() {
                        *tps = Some(f64::from(frames) / seconds);
                    }
                }
                close = true;
            }
            Some(_) => {}
        }
    }
    if close {
        counters.window_frames.store(0, Ordering::Relaxed);
        if let Ok(mut window) = counters.window_started.lock() {
            *window = Some(Instant::now());
        }
    }
}

/// Whether there is a client running this mod at all — which, from inside one, is true.
#[must_use]
pub fn is_running() -> bool {
    true
}

/// How long this mod has been loaded, in whole seconds.
#[must_use]
pub fn uptime_secs() -> u64 {
    started().elapsed().as_secs()
}

/// Frames the loader has seen.
#[must_use]
pub fn frames() -> u64 {
    counters().frames.load(Ordering::Relaxed)
}

/// Ticks the loader has run: one per client frame, because that is when a mod is ticked.
#[must_use]
pub fn ticks() -> u64 {
    counters().ticks.load(Ordering::Relaxed)
}

/// The tick the client is on, as this loader counts them.
#[must_use]
pub fn current_tick() -> u64 {
    ticks()
}

/// Frames per second over the last window, or `None` before a window has closed.
#[must_use]
pub fn current_fps() -> Option<f64> {
    counters().fps.lock().ok().and_then(|fps| *fps)
}

/// Ticks per second, which for this host is the same measurement as the frames above.
#[must_use]
pub fn current_tps() -> Option<f64> {
    counters().tps.lock().ok().and_then(|tps| *tps)
}

/// The longest frame the loader has seen, in milliseconds.
#[must_use]
pub fn slowest_frame_millis() -> u64 {
    counters().slowest_frame_millis.load(Ordering::Relaxed)
}

/// How long since the last frame, in milliseconds. A mod can use this to notice that the
/// client has stopped drawing.
#[must_use]
pub fn since_last_frame_millis() -> Option<u64> {
    counters()
        .last_frame
        .lock()
        .ok()
        .and_then(|last| last.map(|last| last.elapsed().as_millis() as u64))
}

/// The phase the loader is in: `load`, `tick`, `render_start` and so on.
#[must_use]
pub fn current_phase() -> String {
    crate::lifecycle::current_phase().name().to_owned()
}

/// The game version the client's own assets name.
#[must_use]
pub fn get_version() -> String {
    crate::host_facts().game_version
}

/// The protocol number, when the loader knows one.
#[must_use]
pub fn get_protocol_version() -> Option<u32> {
    crate::host_facts().game_protocol
}

/// The address the client was launched with, when there was one.
#[must_use]
pub fn current_server() -> Option<String> {
    crate::host_facts().launch_address
}

/// Whether the client was launched against an address, which is what this loader can tell
/// about single-player and multiplayer. It cannot see whether a session is connected.
#[must_use]
pub fn is_multiplayer() -> bool {
    current_server().is_some()
}

/// Whether the client was launched without an address.
#[must_use]
pub fn is_singleplayer() -> bool {
    !is_multiplayer()
}

/// Whether a session looks live: a server was named and frames are still arriving.
///
/// This is the loader's own observation, and it is called what it is rather than pretending
/// to be the client's connection state.
#[must_use]
pub fn is_online() -> bool {
    is_multiplayer() && since_last_frame_millis().is_some_and(|millis| millis < 1000)
}

/// The window the client is drawing into, from the display the loader reads.
#[must_use]
pub fn get_window() -> Option<Window> {
    crate::host_facts().window.map(|(width, height)| Window {
        width,
        height,
        scale: 1.0,
    })
}

/// How big the client's window is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

/// Everything this loader knows about the session in one value.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionInfo {
    pub version: String,
    pub protocol: Option<u32>,
    pub address: Option<String>,
    pub multiplayer: bool,
    pub uptime_secs: u64,
    pub frames: u64,
    pub ticks: u64,
    pub fps: Option<f64>,
    pub tps: Option<f64>,
    pub slowest_frame_millis: u64,
    pub phase: String,
    pub window: Option<Window>,
    pub language: String,
    pub mods: usize,
}

/// What a caller may ask about this session.
#[must_use]
pub fn get_session_info() -> SessionInfo {
    SessionInfo {
        version: get_version(),
        protocol: get_protocol_version(),
        address: current_server(),
        multiplayer: is_multiplayer(),
        uptime_secs: uptime_secs(),
        frames: frames(),
        ticks: ticks(),
        fps: current_fps(),
        tps: current_tps(),
        slowest_frame_millis: slowest_frame_millis(),
        phase: current_phase(),
        window: get_window(),
        language: crate::i18n::get_current_language(),
        mods: crate::mods::all().len(),
    }
}

/// The process this mod is in: id, threads, ABI, the loader's version.
#[must_use]
pub fn get_runtime_info() -> crate::diagnostics::RuntimeInfo {
    crate::diagnostics::get_runtime_info()
}

/// The settings this host has, which are the loader's own, not the client's.
#[must_use]
pub fn get_settings() -> Vec<(String, String)> {
    let mut settings: Vec<(String, String)> = vec![
        (
            "mods_dir".to_owned(),
            crate::mods_dir().unwrap_or_default().to_owned(),
        ),
    ];
    settings.push(("language".to_owned(), crate::i18n::get_current_language()));
    settings.push((
        "menu_open".to_owned(),
        std::env::var("CINNALOADER_MENU_OPEN").unwrap_or_else(|_| "0".to_owned()),
    ));
    settings.push(("abi".to_owned(), crate::ABI_VERSION.to_string()));
    settings.push((
        "disabled_mods".to_owned(),
        crate::mods::disabled()
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>()
            .join(","),
    ));
    settings
}

/// The loader's resource stack, as a summary: packs, and what wins what.
#[must_use]
pub fn get_resource_manager() -> Vec<(String, String, i32, usize)> {
    crate::resources::get_resource_stack()
}

/// The loader's mod registry, as a summary of what is installed.
#[must_use]
pub fn get_registry_manager() -> Vec<(String, String, bool, bool)> {
    crate::mods::all()
        .into_iter()
        .map(|entry| {
            (
                entry.name,
                entry.id,
                entry.enabled,
                entry.state.is_active(),
            )
        })
        .collect()
}

/// Why a question here has no answer in this host.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "screen" => "the client's screen is an object inside the client, and this host has no handle on it".to_owned(),
        "world" => "the world lives in the client's own state; reading it needs the hooks section 10 lists as never".to_owned(),
        "player" => "the local player is the client's, and this host cannot reach it".to_owned(),
        "dimension" => "the current dimension is part of the client's world state".to_owned(),
        "game_mode" => "the game mode is part of the client's world state".to_owned(),
        "paused" => "the client's pause state is not exposed to this host".to_owned(),
        "menu" => "whether the client is in a menu is the client's own screen state".to_owned(),
        "network" => "the client owns its network manager and exposes no handle to it".to_owned(),
        "renderer" => "the world renderer is the client's, behind the hooks section 10 lists as never".to_owned(),
        "server_object" => "this loader is a client mod host; there is no server object here".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// The client's current screen. `None`, with [`why_unknown`] saying why.
#[must_use]
pub fn current_screen() -> Option<String> {
    None
}

/// The world the client is in. `None`: see [`why_unknown`].
#[must_use]
pub fn current_world() -> Option<String> {
    None
}

/// The local player. `None`: see [`why_unknown`].
#[must_use]
pub fn current_player() -> Option<String> {
    None
}

/// The dimension the client is in. `None`: see [`why_unknown`].
#[must_use]
pub fn current_dimension() -> Option<String> {
    None
}

/// The game mode. `None`: see [`why_unknown`].
#[must_use]
pub fn current_game_mode() -> Option<String> {
    None
}

/// Whether the client is paused. `None`: see [`why_unknown`].
#[must_use]
pub fn is_paused() -> Option<bool> {
    None
}

/// Whether the client is showing a menu. `None`: see [`why_unknown`].
#[must_use]
pub fn is_in_menu() -> Option<bool> {
    None
}

/// Whether the client is in a world. `None`: see [`why_unknown`].
#[must_use]
pub fn is_in_world() -> Option<bool> {
    None
}

/// The client itself, as an object. `None`: see [`why_unknown`].
#[must_use]
pub fn get_client() -> Option<()> {
    None
}

/// A server object. `None`: there is none in a client mod host.
#[must_use]
pub fn get_server() -> Option<()> {
    None
}

/// The client's network manager. `None`: see [`why_unknown`].
#[must_use]
pub fn get_network_manager() -> Option<()> {
    None
}

/// The client's world renderer. `None`: see [`why_unknown`].
#[must_use]
pub fn get_world_renderer() -> Option<()> {
    None
}

/// The world's time of day. `None`: the client's world state is out of reach; the host's own
/// clock is [`now_unix_seconds`].
#[must_use]
pub fn current_time() -> Option<u64> {
    None
}

/// The host's clock, in seconds since the Unix epoch, for a caller that wants a time.
#[must_use]
pub fn now_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// A request a mod may make of the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Disconnect,
    WorldReload,
    ResourceReload,
    GameShutdown,
}

impl fmt::Display for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Disconnect => "disconnect",
            Self::WorldReload => "world reload",
            Self::ResourceReload => "resource reload",
            Self::GameShutdown => "game shutdown",
        })
    }
}

/// What the client was asked to do, and what came of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestOutcome {
    pub request: Request,
    /// Whether the loader could do it.
    pub granted: bool,
    pub detail: String,
}

/// Everything a mod has asked the client to do, oldest first.
#[must_use]
pub fn get_requests() -> Vec<RequestOutcome> {
    requests()
        .lock()
        .map(|requests| requests.clone())
        .unwrap_or_default()
}

fn requests() -> &'static Mutex<Vec<RequestOutcome>> {
    static REQUESTS: OnceLock<Mutex<Vec<RequestOutcome>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(Vec::new()))
}

fn record(request: Request, granted: bool, detail: &str) -> bool {
    if let Ok(mut requests) = requests().lock() {
        requests.push(RequestOutcome {
            request,
            granted,
            detail: detail.to_owned(),
        });
    }
    if granted {
        crate::log::log_fields(
            crate::log::Level::Info,
            "client request",
            &[("request", &request.to_string()), ("granted", "yes")],
        );
    } else {
        crate::warn!("a mod asked the client to {}: {detail}", request);
    }
    granted
}

/// Asks the client to leave its session. This host cannot make it, so the request is
/// recorded and said out loud instead of pretending.
pub fn request_disconnect() -> bool {
    record(
        Request::Disconnect,
        false,
        "the loader has no handle on the client's session; the request is recorded",
    )
}

/// Asks the client to reload its world. Same answer as [`request_disconnect`], and for the
/// same reason.
pub fn request_world_reload() -> bool {
    record(
        Request::WorldReload,
        false,
        "the loader cannot ask the client to reload a world; the request is recorded",
    )
}

/// Asks for a resource reload, which this host *can* do: it reloads the loader's own stack
/// and fires the event.
pub fn request_resource_reload() -> bool {
    let ran = crate::resources::reload();
    record(
        Request::ResourceReload,
        true,
        &format!("{ran} reload listener(s) ran"),
    )
}

/// Asks the client to shut down. This host will not kill a client mid-frame: the request is
/// recorded and the user is told.
pub fn request_game_shutdown() -> bool {
    record(
        Request::GameShutdown,
        false,
        "killing the client mid-frame corrupts its state; the request is recorded for the user",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn frames_and_rates_come_from_the_client_loop() {
        let _serial = serial();
        let before = frames();
        for _ in 0..RATE_WINDOW {
            note_frame(16);
        }
        assert!(frames() >= before + u64::from(RATE_WINDOW));
        assert!(current_fps().is_some(), "a window closed, so there is a rate");
        assert!(current_tps().is_some());
        assert!(slowest_frame_millis() >= 16);
        assert!(since_last_frame_millis().is_some());
        assert!(ticks() >= u64::from(RATE_WINDOW));
    }

    #[test]
    fn what_this_host_cannot_see_says_so() {
        assert!(current_screen().is_none());
        assert!(current_world().is_none());
        assert!(current_player().is_none());
        assert!(current_dimension().is_none());
        assert!(current_game_mode().is_none());
        assert_eq!(is_paused(), None);
        assert_eq!(is_in_menu(), None);
        assert_eq!(is_in_world(), None);
        assert!(get_client().is_none());
        assert!(get_server().is_none());
        assert!(get_network_manager().is_none());
        assert!(get_world_renderer().is_none());
        assert!(current_time().is_none());
        for what in [
            "screen",
            "world",
            "player",
            "dimension",
            "game_mode",
            "paused",
            "menu",
            "network",
            "renderer",
            "server_object",
        ] {
            assert!(!why_unknown(what).is_empty(), "{what} has no reason");
            assert_ne!(why_unknown(what), format!("{what} is not something this host can see"));
        }
    }

    #[test]
    fn the_session_reads_like_something_a_mod_can_use() {
        let _serial = serial();
        let info = get_session_info();
        assert!(info.uptime_secs < 60 * 60);
        assert!(info.multiplayer == info.address.is_some());
        assert!(!info.phase.is_empty());
        assert!(info.mods == crate::mods::all().len());
    }

    #[test]
    fn the_two_managers_this_host_owns_are_reachable() {
        let _serial = serial();
        assert!(get_resource_manager().is_empty(), "no packs registered in a test");
        assert_eq!(get_registry_manager().len(), crate::mods::all().len());
        assert!(get_settings()
            .iter()
            .any(|(key, _)| key == "mods_dir"));
    }

    #[test]
    fn a_request_the_host_cannot_make_is_recorded_rather_than_faked() {
        let _serial = serial();
        let before = get_requests().len();
        assert!(!request_disconnect());
        assert!(!request_world_reload());
        assert!(!request_game_shutdown());
        assert!(request_resource_reload(), "this one the host can do");
        let requests = get_requests();
        assert_eq!(requests.len(), before + 4);
        assert!(requests.last().is_some_and(|outcome| outcome.granted));
        assert!(requests
            .iter()
            .any(|outcome| outcome.request == Request::Disconnect && !outcome.granted));
    }

    #[test]
    fn the_host_clock_answers_even_where_the_world_clock_cannot() {
        assert!(current_time().is_none());
        assert!(now_unix_seconds() > 1_700_000_000);
    }
}
