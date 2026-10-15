//! Time: the arithmetic a mod can run itself, the clock it owns, and the client's clock it
//! cannot read.
//!
//! The unit is the tick, and the conversion is fixed: **20 ticks per second, 50 ms per
//! tick**, so a day of 24 000 ticks is 1 200 000 ms. [`millis_to_ticks`] and [`seconds_to_ticks`]
//! round to the **nearest** tick, and exactly half a tick rounds **up** (25 ms is one tick,
//! 24 ms is none).
//!
//! A [`Clock`] is a mod-owned counter of ticks. It answers the total ticks and milliseconds,
//! the day number (`ticks / 24 000`), the time of day (`ticks % 24 000`) and the [`Phase`] the
//! time of day falls in. The day runs `0..24 000` from midnight, so `0` is `00:00`, `6 000` is
//! `06:00` and the phase boundaries are: night `0..5 000`, sunrise `5 000..6 000`, day
//! `6 000..17 000`, sunset `17 000..18 000`, then night again. `is_day`/`is_night` agree with
//! those boundaries, and [`Phase::start_tick`] is the inverse of [`Phase::at`].
//!
//! A [`Scheduler`] runs callbacks against a virtual time it advances by a delta: a one-shot
//! fires at `now + delay`, a repeating task reschedules **from its own due time** (so it does
//! not drift), and `advance` fires exactly the tasks that are due and no others. A
//! [`Stopwatch`] holds its total while stopped; a [`Timer`] fires once or accumulates per
//! interval. The client's own world clock, its time of day and its tick counter are refused in
//! [`why_unknown`], listed by [`refusals`], and never faked.
//!
//! ```
//! use cinnaloader::time::{self, Clock, Phase};
//!
//! assert_eq!(time::ticks_to_millis(20), 1_000);
//! assert_eq!(time::millis_to_ticks(1_000), 20);
//! assert_eq!(time::format_duration(3_723_000), "1h 2m 3s");
//! assert_eq!(time::format_time_of_day(6_000), "06:00");
//!
//! let mut clock = Clock::new();
//! clock.advance(6_000);
//! assert_eq!(clock.time_of_day(), 6_000);
//! assert_eq!(clock.phase(), Phase::Day);
//! assert_eq!(clock.hour(), 6);
//! assert!(clock.is_day());
//!
//! let mut watch = time::create_stopwatch();
//! watch.advance(100);
//! assert_eq!(watch.elapsed(), 0, "not running yet");
//! watch.start();
//! watch.advance(100);
//! assert_eq!(watch.elapsed(), 100);
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Ticks in one second. The client's fixed rate.
pub const TICKS_PER_SECOND: u64 = 20;
/// Milliseconds in one tick, which is `1 000 / 20`.
pub const MILLIS_PER_TICK: u64 = 50;
/// Milliseconds in one second.
pub const MILLIS_PER_SECOND: u64 = 1_000;
/// Ticks in one day.
pub const DAY_TICKS: u32 = 24_000;
/// Milliseconds in one day, which is `24 000 * 50`.
pub const DAY_MILLIS: u64 = 1_200_000;

/// Ticks to milliseconds.
#[must_use]
pub fn ticks_to_millis(ticks: u64) -> u64 {
    ticks.saturating_mul(MILLIS_PER_TICK)
}

/// Milliseconds to ticks, to the nearest tick; exactly half a tick rounds up.
#[must_use]
pub fn millis_to_ticks(millis: u64) -> u64 {
    (millis + MILLIS_PER_TICK / 2) / MILLIS_PER_TICK
}

/// Ticks to seconds, as a fraction.
#[must_use]
pub fn ticks_to_seconds(ticks: u64) -> f64 {
    ticks as f64 / TICKS_PER_SECOND as f64
}

/// Seconds to ticks, to the nearest tick; exactly half a tick rounds up. A negative or
/// non-finite input is no ticks.
#[must_use]
pub fn seconds_to_ticks(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    (seconds * TICKS_PER_SECOND as f64).round() as u64
}

/// Ticks as a [`Duration`].
#[must_use]
pub fn to_duration(ticks: u64) -> Duration {
    Duration::from_millis(ticks_to_millis(ticks))
}

/// A [`Duration`] as ticks, rounded to the nearest tick like [`millis_to_ticks`].
#[must_use]
pub fn from_duration(duration: Duration) -> u64 {
    millis_to_ticks(duration.as_millis() as u64)
}

/// Where a time of day falls in the day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Full daylight.
    Day,
    /// The sun is going down.
    Sunset,
    /// Full dark.
    Night,
    /// The sun is coming up.
    Sunrise,
}

/// The tick the evening begins at, where [`Phase::Night`] starts in the day's own range.
const SUNRISE_TICK: u32 = 5_000;
const DAY_START_TICK: u32 = 6_000;
const SUNSET_TICK: u32 = 17_000;
const NIGHT_TICK: u32 = 18_000;

impl Phase {
    /// The phase a time of day falls in, over `0..24 000`.
    #[must_use]
    pub fn at(time_of_day: u32) -> Self {
        let tod = time_of_day % DAY_TICKS;
        if tod < SUNRISE_TICK {
            Self::Night
        } else if tod < DAY_START_TICK {
            Self::Sunrise
        } else if tod < SUNSET_TICK {
            Self::Day
        } else if tod < NIGHT_TICK {
            Self::Sunset
        } else {
            Self::Night
        }
    }

    /// The tick this phase starts at: night at `18 000` (its evening edge), sunrise at
    /// `5 000`, day at `6 000`, sunset at `17 000`.
    #[must_use]
    pub fn start_tick(self) -> u32 {
        match self {
            Self::Day => DAY_START_TICK,
            Self::Sunset => SUNSET_TICK,
            Self::Night => NIGHT_TICK,
            Self::Sunrise => SUNRISE_TICK,
        }
    }

    /// Whether the sun is up in this phase.
    #[must_use]
    pub fn is_day(self) -> bool {
        !matches!(self, Self::Night)
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Sunset => "sunset",
            Self::Night => "night",
            Self::Sunrise => "sunrise",
        }
    }
}

/// A mod-owned counter of ticks: total ticks, a day number and a time of day.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clock {
    ticks: u64,
}

impl Clock {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A clock that starts at `ticks`.
    #[must_use]
    pub fn from_ticks(ticks: u64) -> Self {
        Self { ticks }
    }

    /// Advances by `ticks`, wrapping the time of day and rolling the day over at midnight.
    pub fn advance(&mut self, ticks: u64) {
        self.ticks = self.ticks.saturating_add(ticks);
    }

    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub fn millis(&self) -> u64 {
        ticks_to_millis(self.ticks)
    }

    #[must_use]
    pub fn seconds(&self) -> f64 {
        ticks_to_seconds(self.ticks)
    }

    /// Days since the clock started, one per `24 000` ticks.
    #[must_use]
    pub fn day(&self) -> u64 {
        self.ticks / u64::from(DAY_TICKS)
    }

    /// Ticks since the last midnight, over `0..24 000`.
    #[must_use]
    pub fn time_of_day(&self) -> u32 {
        (self.ticks % u64::from(DAY_TICKS)) as u32
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        Phase::at(self.time_of_day())
    }

    /// The hour the clock reads, `0..24` from midnight.
    #[must_use]
    pub fn hour(&self) -> u32 {
        self.time_of_day() / 1_000
    }

    /// The minute within the hour, `0..60`.
    #[must_use]
    pub fn minute(&self) -> u32 {
        (self.time_of_day() % 1_000) * 60 / 1_000
    }

    /// The time of day as a fraction of the day, `0.0..1.0`.
    #[must_use]
    pub fn fraction(&self) -> f64 {
        f64::from(self.time_of_day()) / f64::from(DAY_TICKS)
    }

    #[must_use]
    pub fn is_day(&self) -> bool {
        self.phase().is_day()
    }

    #[must_use]
    pub fn is_night(&self) -> bool {
        !self.is_day()
    }

    /// Moves within the current day to `time_of_day`, wrapping into `0..24 000`.
    pub fn set_time_of_day(&mut self, time_of_day: u32) {
        self.ticks = self.day() * u64::from(DAY_TICKS) + u64::from(time_of_day % DAY_TICKS);
    }
}

fn clocks() -> &'static Mutex<HashMap<u64, (String, Clock)>> {
    static CLOCKS: OnceLock<Mutex<HashMap<u64, (String, Clock)>>> = OnceLock::new();
    CLOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_CLOCK: AtomicU64 = AtomicU64::new(1);

/// Registers a named clock starting at midnight and returns its id.
pub fn register_clock(name: &str) -> u64 {
    let id = NEXT_CLOCK.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut clocks) = clocks().lock() {
        clocks.insert(id, (name.to_owned(), Clock::new()));
    }
    id
}

/// The clock registered under `id`, or `None` when there is none.
#[must_use]
pub fn get_clock(id: u64) -> Option<Clock> {
    clocks()
        .lock()
        .ok()
        .and_then(|clocks| clocks.get(&id).map(|(_, clock)| *clock))
}

/// The name a clock was registered under.
#[must_use]
pub fn clock_name(id: u64) -> Option<String> {
    clocks()
        .lock()
        .ok()
        .and_then(|clocks| clocks.get(&id).map(|(name, _)| name.clone()))
}

/// Advances a registered clock and returns its new reading.
pub fn advance_clock(id: u64, ticks: u64) -> Option<Clock> {
    let mut clocks = clocks().lock().ok()?;
    let (_, clock) = clocks.get_mut(&id)?;
    clock.advance(ticks);
    Some(*clock)
}

/// Removes a registered clock; false when there was none.
pub fn unregister_clock(id: u64) -> bool {
    clocks()
        .lock()
        .map(|mut clocks| clocks.remove(&id).is_some())
        .unwrap_or(false)
}

/// How many clocks are registered.
#[must_use]
pub fn clock_count() -> usize {
    clocks().lock().map(|clocks| clocks.len()).unwrap_or(0)
}

type Run = Box<dyn FnMut() + Send>;

struct Job {
    id: u64,
    due: u64,
    every: Option<u64>,
    run: Run,
}

/// A scheduler a mod owns: callbacks that fire against a time `advance` moves forward.
#[derive(Default)]
pub struct Scheduler {
    now: u64,
    next: u64,
    jobs: Vec<Job>,
}

impl Scheduler {
    #[must_use]
    pub fn new() -> Self {
        Self {
            now: 0,
            next: 1,
            jobs: Vec::new(),
        }
    }

    /// The scheduler's current time, in ticks.
    #[must_use]
    pub fn at(&self) -> u64 {
        self.now
    }

    /// Runs `run` once, `delay` ticks from now, and returns its id.
    pub fn after(&mut self, delay: u64, run: impl FnMut() + Send + 'static) -> u64 {
        let id = self.next;
        self.next += 1;
        self.jobs.push(Job {
            id,
            due: self.now.saturating_add(delay),
            every: None,
            run: Box::new(run),
        });
        id
    }

    /// Runs `run` every `interval` ticks from now, and returns its id. An interval of zero is
    /// treated as one.
    pub fn every(&mut self, interval: u64, run: impl FnMut() + Send + 'static) -> u64 {
        let id = self.next;
        self.next += 1;
        let every = interval.max(1);
        self.jobs.push(Job {
            id,
            due: self.now.saturating_add(every),
            every: Some(every),
            run: Box::new(run),
        });
        id
    }

    /// Moves the clock forward by `delta` and runs everything that comes due, in due order.
    /// A repeating task reschedules from its own due time, so it does not drift. Returns how
    /// many times a callback ran.
    pub fn advance(&mut self, delta: u64) -> usize {
        self.now = self.now.saturating_add(delta);
        let mut fired = 0;
        loop {
            let mut earliest: Option<usize> = None;
            for (index, job) in self.jobs.iter().enumerate() {
                if job.due > self.now {
                    continue;
                }
                if earliest.is_none_or(|at| job.due < self.jobs[at].due) {
                    earliest = Some(index);
                }
            }
            let Some(index) = earliest else {
                break;
            };
            let every = self.jobs[index].every;
            (self.jobs[index].run)();
            fired += 1;
            if let Some(every) = every {
                self.jobs[index].due += every;
            } else {
                self.jobs.remove(index);
            }
        }
        fired
    }

    /// Takes a task off the queue; false when it had already fired and does not repeat.
    pub fn cancel(&mut self, id: u64) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|job| job.id != id);
        self.jobs.len() != before
    }

    /// How many tasks are still queued.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.jobs.len()
    }

    /// When a queued task is next due, in ticks.
    #[must_use]
    pub fn due_at(&self, id: u64) -> Option<u64> {
        self.jobs.iter().find(|job| job.id == id).map(|job| job.due)
    }
}

fn scheduler() -> &'static Mutex<Scheduler> {
    static SCHEDULER: OnceLock<Mutex<Scheduler>> = OnceLock::new();
    SCHEDULER.get_or_init(|| Mutex::new(Scheduler::new()))
}

/// Runs `run` once, `delay` ticks from now, on the host's scheduler; returns its id.
pub fn schedule(delay: u64, run: impl FnMut() + Send + 'static) -> u64 {
    scheduler()
        .lock()
        .map(|mut scheduler| scheduler.after(delay, run))
        .unwrap_or(0)
}

/// Runs `run` every `interval` ticks, on the host's scheduler; returns its id.
pub fn schedule_repeating(interval: u64, run: impl FnMut() + Send + 'static) -> u64 {
    scheduler()
        .lock()
        .map(|mut scheduler| scheduler.every(interval, run))
        .unwrap_or(0)
}

/// Registers a handler that runs on every tick of the host's scheduler.
pub fn register_tick_handler(run: impl FnMut() + Send + 'static) -> u64 {
    schedule_repeating(1, run)
}

/// Moves the host's scheduler forward, firing what comes due.
pub fn advance(delta: u64) -> usize {
    scheduler()
        .lock()
        .map(|mut scheduler| scheduler.advance(delta))
        .unwrap_or(0)
}

/// Takes a task off the host's scheduler; see [`Scheduler::cancel`].
pub fn cancel(id: u64) -> bool {
    scheduler()
        .lock()
        .map(|mut scheduler| scheduler.cancel(id))
        .unwrap_or(false)
}

/// How many tasks are queued on the host's scheduler.
#[must_use]
pub fn pending() -> usize {
    scheduler()
        .lock()
        .map(|scheduler| scheduler.pending())
        .unwrap_or(0)
}

/// A stopwatch that measures ticks while running and holds its total while stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stopwatch {
    running: bool,
    elapsed: u64,
}

impl Stopwatch {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&mut self) {
        self.running = true;
    }

    pub fn stop(&mut self) {
        self.running = false;
    }

    pub fn reset(&mut self) {
        self.running = false;
        self.elapsed = 0;
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Adds `ticks` to the total, but only while running.
    pub fn advance(&mut self, ticks: u64) {
        if self.running {
            self.elapsed = self.elapsed.saturating_add(ticks);
        }
    }

    #[must_use]
    pub fn elapsed(&self) -> u64 {
        self.elapsed
    }

    #[must_use]
    pub fn elapsed_millis(&self) -> u64 {
        ticks_to_millis(self.elapsed)
    }

    #[must_use]
    pub fn elapsed_seconds(&self) -> f64 {
        ticks_to_seconds(self.elapsed)
    }
}

/// A timer that fires once, or one that accumulates and fires every interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    period: u64,
    acc: u64,
    repeat: bool,
    done: bool,
    fired: u64,
}

impl Timer {
    /// A one-shot timer: fires once, `period` ticks from the first `advance`.
    #[must_use]
    pub fn new(period: u64) -> Self {
        Self {
            period: period.max(1),
            acc: 0,
            repeat: false,
            done: false,
            fired: 0,
        }
    }

    /// A timer that fires every `period` ticks, keeping the remainder so it does not drift.
    #[must_use]
    pub fn repeating(period: u64) -> Self {
        Self {
            period: period.max(1),
            acc: 0,
            repeat: true,
            done: false,
            fired: 0,
        }
    }

    /// Adds `delta` to the accumulator and returns how many times it fired.
    pub fn advance(&mut self, delta: u64) -> u64 {
        if self.done {
            return 0;
        }
        let period = self.period.max(1);
        self.acc = self.acc.saturating_add(delta);
        if self.repeat {
            let fires = self.acc / period;
            self.acc %= period;
            self.fired += fires;
            fires
        } else if self.acc >= period {
            self.acc = period;
            self.done = true;
            self.fired += 1;
            1
        } else {
            0
        }
    }

    #[must_use]
    pub fn fired(&self) -> u64 {
        self.fired
    }

    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }

    #[must_use]
    pub fn period(&self) -> u64 {
        self.period
    }

    /// Ticks still accumulating before the next fire.
    #[must_use]
    pub fn accumulated(&self) -> u64 {
        self.acc
    }

    /// Fires once more from zero.
    pub fn reset(&mut self) {
        self.acc = 0;
        self.done = false;
    }
}

/// A stopwatch, for a caller that wants the wishlist's name.
#[must_use]
pub fn create_stopwatch() -> Stopwatch {
    Stopwatch::new()
}

/// A one-shot timer, for a caller that wants the wishlist's name.
#[must_use]
pub fn create_timer(period: u64) -> Timer {
    Timer::new(period)
}

/// A duration as text: `500 ms`, then the non-zero `d`/`h`/`m`/`s` parts, joined by a space.
/// A whole-second duration drops nothing above it; below a second it is milliseconds.
#[must_use]
pub fn format_duration(millis: u64) -> String {
    let seconds = millis / MILLIS_PER_SECOND;
    if seconds == 0 {
        return format!("{millis} ms");
    }
    let days = seconds / 86_400;
    let hours = (seconds / 3_600) % 24;
    let minutes = (seconds / 60) % 60;
    let secs = seconds % 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if secs > 0 {
        parts.push(format!("{secs}s"));
    }
    parts.join(" ")
}

/// A time of day as `HH:MM`, over `0..24 000` ticks from midnight.
#[must_use]
pub fn format_time_of_day(time_of_day: u32) -> String {
    let tod = time_of_day % DAY_TICKS;
    format!("{:02}:{:02}", tod / 1_000, (tod % 1_000) * 60 / 1_000)
}

fn started() -> &'static Instant {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    STARTED.get_or_init(Instant::now)
}

fn tick_rate() -> &'static AtomicU32 {
    static RATE: AtomicU32 = AtomicU32::new(TICKS_PER_SECOND as u32);
    &RATE
}

/// Milliseconds in one tick at the current tick rate.
fn millis_per_tick() -> u64 {
    MILLIS_PER_SECOND / u64::from(get_tick_rate().max(1))
}

/// The wall clock, in milliseconds since the Unix epoch.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// The monotonic clock, in milliseconds since this host started. Never goes backwards.
#[must_use]
pub fn monotonic_now() -> u64 {
    started().elapsed().as_millis() as u64
}

/// Seconds in one frame. This host ticks once per frame, so it is the tick duration.
#[must_use]
pub fn delta_time() -> f32 {
    millis_per_tick() as f32 / MILLIS_PER_SECOND as f32
}

/// The duration of one frame, which is one tick.
#[must_use]
pub fn frame_time() -> Duration {
    tick_time()
}

/// The duration of one tick at the current tick rate.
#[must_use]
pub fn tick_time() -> Duration {
    Duration::from_millis(millis_per_tick())
}

/// Ticks this host has run, from its own monotonic clock.
#[must_use]
pub fn game_time() -> u64 {
    monotonic_now() / millis_per_tick().max(1)
}

/// The client's world clock. `None`: this host has no handle on the client's world state.
#[must_use]
pub fn world_time() -> Option<u64> {
    crate::warn!(
        "a mod asked for the client's world time: {}",
        why_unknown("world_time")
    );
    None
}

/// The client's time of day. `None`: see [`why_unknown`].
#[must_use]
pub fn day_time() -> Option<u32> {
    crate::warn!(
        "a mod asked for the client's time of day: {}",
        why_unknown("day_time")
    );
    None
}

/// The client's tick counter. `None`: see [`why_unknown`].
#[must_use]
pub fn get_tick_count() -> Option<u64> {
    crate::warn!(
        "a mod asked for the client's tick counter: {}",
        why_unknown("get_tick_count")
    );
    None
}

/// The tick rate this host converts with.
#[must_use]
pub fn get_tick_rate() -> u32 {
    tick_rate().load(Ordering::Relaxed)
}

/// Sets the tick rate, treating zero as one, and returns what took.
pub fn set_tick_rate(rate: u32) -> u32 {
    let rate = rate.max(1);
    tick_rate().store(rate, Ordering::Relaxed);
    rate
}

/// The last frame rate the loader measured, or `None` before a rate window closes.
#[must_use]
pub fn get_frame_rate() -> Option<f64> {
    crate::game::current_fps()
}

/// How long this host has been running, in whole seconds.
#[must_use]
pub fn get_uptime() -> u64 {
    monotonic_now() / MILLIS_PER_SECOND
}

/// How long this host has been running, in milliseconds.
#[must_use]
pub fn get_elapsed() -> u64 {
    monotonic_now()
}

/// Sleeps for `duration`. Real time, for a worker or a startup path.
pub fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

/// The host's timezone name. The loader keeps UTC and does not read the machine's zone.
#[must_use]
pub fn get_timezone() -> &'static str {
    "UTC"
}

/// The wall clock, in seconds since the Unix epoch.
#[must_use]
pub fn get_timestamp() -> u64 {
    now().max(0) as u64 / MILLIS_PER_SECOND
}

/// Frames the loader has counted, one per tick.
#[must_use]
pub fn get_frame_count() -> u64 {
    crate::game::frames()
}

/// How far into the current tick this moment is, `0.0..1.0`, for interpolating a render.
#[must_use]
pub fn interpolation_alpha() -> f32 {
    let per_tick = millis_per_tick().max(1);
    (monotonic_now() % per_tick) as f32 / per_tick as f32
}

/// Why a clock question has no answer in this host.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "world_time" => {
            "the client's world clock lives in its own state, and this host has no handle on \
             it"
                .to_owned()
        }
        "day_time" => {
            "the client's time of day is part of its world state, which this host cannot read"
                .to_owned()
        }
        "get_tick_count" => {
            "the client's tick counter is stepped inside the client, where this host cannot see \
             it"
                .to_owned()
        }
        _ => format!("{what} is not a clock this host can read"),
    }
}

/// What this host cannot read about the client's clock.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["world_time", "day_time", "get_tick_count"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[test]
    fn conversions_round_trip_and_a_half_tick_rounds_up() {
        assert_eq!(TICKS_PER_SECOND, 20);
        assert_eq!(MILLIS_PER_TICK, 50);
        assert_eq!(DAY_MILLIS, 1_200_000);
        assert_eq!(ticks_to_millis(20), 1_000);
        assert_eq!(millis_to_ticks(1_000), 20);
        assert_eq!(ticks_to_seconds(30), 1.5);
        assert_eq!(seconds_to_ticks(1.5), 30);
        assert_eq!(millis_to_ticks(25), 1, "exactly half a tick rounds up");
        assert_eq!(millis_to_ticks(24), 0, "under half a tick rounds down");
        assert_eq!(seconds_to_ticks(-1.0), 0);
        assert_eq!(seconds_to_ticks(f64::NAN), 0);
        for ticks in [0u64, 1, 7, 20, 1_000, 24_000] {
            assert_eq!(millis_to_ticks(ticks_to_millis(ticks)), ticks);
            assert_eq!(from_duration(to_duration(ticks)), ticks);
        }
        assert_eq!(to_duration(7), Duration::from_millis(350));
    }

    #[test]
    fn a_clock_advances_by_a_delta_and_crosses_midnight() {
        let mut clock = Clock::new();
        clock.advance(13_000);
        assert_eq!(clock.ticks(), 13_000);
        assert_eq!(clock.day(), 0);
        assert_eq!(clock.time_of_day(), 13_000);
        assert_eq!(clock.millis(), 650_000);

        // 13 000 + 11 500 = 24 500, one tick past the day boundary.
        clock.advance(11_500);
        assert_eq!(clock.ticks(), 24_500);
        assert_eq!(clock.millis(), 1_225_000);
        assert_eq!(clock.seconds(), 1_225.0);
        assert_eq!(clock.day(), 1);
        assert_eq!(clock.time_of_day(), 500);
        clock.set_time_of_day(0);
        assert_eq!(clock.time_of_day(), 0);
        assert_eq!(clock.day(), 1);
    }

    #[test]
    fn the_day_cycle_bounds_and_the_inverse_round_trip() {
        assert_eq!(Phase::at(0), Phase::Night);
        assert_eq!(Phase::at(4_999), Phase::Night);
        assert_eq!(Phase::at(5_000), Phase::Sunrise);
        assert_eq!(Phase::at(5_999), Phase::Sunrise);
        assert_eq!(Phase::at(6_000), Phase::Day);
        assert_eq!(Phase::at(16_999), Phase::Day);
        assert_eq!(Phase::at(17_000), Phase::Sunset);
        assert_eq!(Phase::at(17_999), Phase::Sunset);
        assert_eq!(Phase::at(18_000), Phase::Night);
        assert_eq!(Phase::at(23_999), Phase::Night);
        for phase in [Phase::Day, Phase::Sunset, Phase::Night, Phase::Sunrise] {
            assert_eq!(Phase::at(phase.start_tick()), phase, "the inverse round trips");
        }

        let night = Clock::from_ticks(18_500);
        assert_eq!(night.hour(), 18);
        assert_eq!(night.minute(), 30);
        assert_eq!(night.phase(), Phase::Night);
        assert!(night.is_night());
        assert!(!night.is_day());
        assert!((night.fraction() - 0.770_833_33).abs() < 1e-6);

        let day = Clock::from_ticks(6_000);
        assert_eq!(day.hour(), 6);
        assert_eq!(day.minute(), 0);
        assert_eq!(day.phase(), Phase::Day);
        assert!(day.is_day());
        assert!(!day.is_night());
        assert_eq!(day.fraction(), 0.25);
    }

    #[test]
    fn a_scheduler_fires_only_what_is_due() {
        let mut scheduler = Scheduler::new();
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let a = Arc::clone(&first);
        scheduler.after(100, move || {
            a.fetch_add(1, Ordering::SeqCst);
        });
        let b = Arc::clone(&second);
        scheduler.after(300, move || {
            b.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(scheduler.pending(), 2);
        assert_eq!(scheduler.advance(150), 1);
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 0, "not due until 300");
        assert_eq!(scheduler.pending(), 1);
        assert_eq!(scheduler.advance(200), 1);
        assert_eq!(second.load(Ordering::SeqCst), 1);
        assert_eq!(scheduler.pending(), 0);
    }

    #[test]
    fn a_repeating_task_keeps_its_own_due_time_without_drift() {
        let mut scheduler = Scheduler::new();
        let beats = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&beats);
        let id = scheduler.every(100, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(scheduler.advance(250), 2, "due at 100 and 200");
        assert_eq!(beats.load(Ordering::SeqCst), 2);
        assert_eq!(
            scheduler.due_at(id),
            Some(300),
            "rescheduled from 200, not from now (250)"
        );
        assert_eq!(scheduler.advance(50), 1);
        assert_eq!(beats.load(Ordering::SeqCst), 3);
        assert_eq!(scheduler.advance(0), 0);
        assert!(scheduler.cancel(id));
        assert_eq!(scheduler.pending(), 0);
        assert_eq!(scheduler.advance(1_000), 0);
    }

    #[test]
    fn a_stopwatch_holds_while_stopped_and_a_timer_fires_once() {
        let mut watch = Stopwatch::new();
        watch.advance(100);
        assert_eq!(watch.elapsed(), 0, "not running");
        watch.start();
        watch.advance(100);
        assert_eq!(watch.elapsed(), 100);
        assert_eq!(watch.elapsed_millis(), 5_000);
        watch.stop();
        watch.advance(500);
        assert_eq!(watch.elapsed(), 100, "holds its total while stopped");
        watch.start();
        watch.advance(50);
        assert_eq!(watch.elapsed(), 150);
        watch.reset();
        assert_eq!(watch.elapsed(), 0);
        assert!(!watch.is_running());

        let mut once = Timer::new(50);
        assert_eq!(once.advance(20), 0);
        assert!(!once.is_done());
        assert_eq!(once.advance(40), 1);
        assert!(once.is_done());
        assert_eq!(once.advance(1_000), 0, "a one-shot fires once");
        assert_eq!(once.fired(), 1);

        let mut every = Timer::repeating(50);
        assert_eq!(every.advance(120), 2);
        assert_eq!(every.fired(), 2);
        assert_eq!(every.accumulated(), 20);
        assert_eq!(every.advance(30), 1);
        assert_eq!(every.fired(), 3);
        assert_eq!(every.period(), 50);
    }

    #[test]
    fn a_duration_and_a_time_of_day_format_exactly() {
        assert_eq!(format_duration(0), "0 ms");
        assert_eq!(format_duration(500), "500 ms");
        assert_eq!(format_duration(1_000), "1s");
        assert_eq!(format_duration(90_000), "1m 30s");
        assert_eq!(format_duration(3_723_000), "1h 2m 3s");
        assert_eq!(format_duration(90_061_000), "1d 1h 1m 1s");
        assert_eq!(format_time_of_day(0), "00:00");
        assert_eq!(format_time_of_day(6_000), "06:00");
        assert_eq!(format_time_of_day(18_500), "18:30");
        assert_eq!(format_time_of_day(23_999), "23:59");
    }

    #[test]
    fn the_host_scheduler_and_clock_registry_serialize() {
        let _serial = crate::test_lock::serial();
        let pending_before = pending();
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        let id = schedule(100, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(pending(), pending_before + 1);
        advance(150);
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        assert_eq!(pending(), pending_before);
        assert!(!cancel(id), "it already fired");

        let clock_id = register_clock("hello:day");
        assert_eq!(clock_name(clock_id).as_deref(), Some("hello:day"));
        assert!(get_clock(clock_id).is_some());
        let advanced = advance_clock(clock_id, 25_000).expect("the clock is registered");
        assert_eq!(advanced.day(), 1);
        assert_eq!(advanced.time_of_day(), 1_000);
        assert_eq!(clock_count(), 1);
        assert!(unregister_clock(clock_id));
        assert!(get_clock(clock_id).is_none());
        assert_eq!(clock_count(), 0);
    }

    #[test]
    fn the_clients_clock_is_refused_not_faked() {
        assert!(world_time().is_none());
        assert!(day_time().is_none());
        assert!(get_tick_count().is_none());
        assert_eq!(refusals().len(), 3);
        for (what, why) in refusals() {
            assert_eq!(why_unknown(&what), why);
            assert!(why.contains("client"), "{why}");
        }
        assert!(why_unknown("world_time").contains("world clock"));
        assert!(why_unknown("day_time").contains("time of day"));
        assert!(why_unknown("get_tick_count").contains("tick counter"));
        assert!(why_unknown("something_else").contains("something_else"));
    }
}
