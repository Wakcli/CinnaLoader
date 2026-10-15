//! Where a mod's time goes: nested sections, a frame sampler and a report, all caller-timed.
//!
//! Every duration here is supplied by the caller — [`end_section`] takes the section's
//! elapsed time, [`record_frame`] takes a frame's — so no wall clock is read and a test can
//! assert exact numbers. [`begin_section`] and [`end_section`] are paired by a stack, so a
//! nested section is attributed to its parent: a section's `total` includes its children and
//! its `self_time` does not, which is what tells a hot section from a merely enclosing one.
//! [`get_hotspots`] ranks by `self_time` and [`get_mod_cost`] sums the totals.
//!
//! [`record_frame`] fills a ring of frame durations that answers the current, mean, minimum
//! and maximum frame time and the percentiles, with the same nearest-rank method as
//! [`crate::metrics`]. A frame strictly above the threshold set by
//! [`set_slow_frame_threshold`] is listed by [`get_slow_frames`].
//!
//! The client's own frame timings, its memory and its profiler output are refused rather than
//! guessed: each has a sentence in [`why_unknown`], the whole set is [`refusals`], and each
//! entry point logs a warning as it answers `None` or empty.
//!
//! ```
//! use cinnaloader::profiler;
//! use std::time::Duration;
//!
//! profiler::reset();
//! profiler::begin_section("tick");
//! profiler::begin_section("ai");
//! profiler::end_section(Duration::from_millis(4));
//! profiler::end_section(Duration::from_millis(10));
//!
//! let tick = profiler::get_section("tick").expect("a section");
//! assert_eq!(tick.total, Duration::from_millis(10));
//! assert_eq!(tick.self_time, Duration::from_millis(6), "its child is excluded");
//!
//! profiler::record_frame(Duration::from_millis(20));
//! profiler::record_frame(Duration::from_millis(40));
//! assert_eq!(profiler::get_current_frame_time(), Some(Duration::from_millis(40)));
//! assert_eq!(profiler::get_average(), Some(Duration::from_millis(30)));
//! assert_eq!(profiler::get_maximum(), Some(Duration::from_millis(40)));
//! ```

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::serialization::{self, Map, Value};

/// The default capacity of the frame ring.
pub const FRAME_WINDOW: usize = 240;

/// One section's aggregate, with self time told apart from the total that includes children.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Section {
    pub name: String,
    pub calls: u64,
    pub total: Duration,
    pub self_time: Duration,
    pub min: Duration,
    pub max: Duration,
}

struct Frame {
    name: String,
    children: Duration,
}

struct Profile {
    running: bool,
    paused: bool,
    sampling: u64,
    stack: Vec<Frame>,
    span_stack: Vec<String>,
    sections: Vec<Section>,
    spans: Vec<(String, Duration)>,
    frames: VecDeque<Duration>,
    slow_threshold: Duration,
    markers: Vec<String>,
    traces: Vec<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            running: true,
            paused: false,
            sampling: 1,
            stack: Vec::new(),
            span_stack: Vec::new(),
            sections: Vec::new(),
            spans: Vec::new(),
            frames: VecDeque::with_capacity(FRAME_WINDOW),
            slow_threshold: Duration::from_millis(50),
            markers: Vec::new(),
            traces: Vec::new(),
        }
    }
}

impl Profile {
    fn recording(&self) -> bool {
        self.running && !self.paused
    }

    fn add_section(&mut self, name: &str, elapsed: Duration, self_time: Duration) {
        if let Some(section) = self.sections.iter_mut().find(|section| section.name == name) {
            section.calls += 1;
            section.total += elapsed;
            section.self_time += self_time;
            section.min = section.min.min(elapsed);
            section.max = section.max.max(elapsed);
            return;
        }
        self.sections.push(Section {
            name: name.to_owned(),
            calls: 1,
            total: elapsed,
            self_time,
            min: elapsed,
            max: elapsed,
        });
    }
}

static NEXT_MARKER: AtomicU64 = AtomicU64::new(1);
static NEXT_TRACE: AtomicU64 = AtomicU64::new(1);

fn profile() -> &'static Mutex<Profile> {
    static PROFILE: OnceLock<Mutex<Profile>> = OnceLock::new();
    PROFILE.get_or_init(|| Mutex::new(Profile::default()))
}

fn with<T>(body: impl FnOnce(&mut Profile) -> T) -> T {
    let mut guard = profile().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Starts collecting. Collection is on by default, so this is for after a [`stop`].
pub fn start() {
    with(|profile| {
        profile.running = true;
        profile.paused = false;
    });
}

/// Stops collecting. Sections begun afterwards are paired but not recorded.
pub fn stop() {
    with(|profile| profile.running = false);
}

/// Pauses collection without giving up the nested stack.
pub fn pause() {
    with(|profile| {
        if profile.running {
            profile.paused = true;
        }
    });
}

/// Resumes after a [`pause`].
pub fn resume() {
    with(|profile| {
        if profile.running {
            profile.paused = false;
        }
    });
}

/// Clears every section, span, frame and marker, and starts fresh.
pub fn reset() {
    with(|profile| {
        profile.stack.clear();
        profile.span_stack.clear();
        profile.sections.clear();
        profile.spans.clear();
        profile.frames.clear();
        profile.markers.clear();
        profile.traces.clear();
        profile.running = true;
        profile.paused = false;
    });
}

/// Whether collection is on.
#[must_use]
pub fn is_running() -> bool {
    with(|profile| profile.running)
}

/// Whether collection is paused.
#[must_use]
pub fn is_paused() -> bool {
    with(|profile| profile.paused)
}

/// Sets how often samples are taken, treating zero as one, and answers what took.
pub fn set_sampling_rate(rate: u64) -> u64 {
    with(|profile| {
        profile.sampling = rate.max(1);
        profile.sampling
    })
}

/// The current sampling rate.
#[must_use]
pub fn get_sampling_rate() -> u64 {
    with(|profile| profile.sampling)
}

/// Registers a marker name and answers its id.
pub fn register_marker(name: &str) -> u64 {
    with(|profile| {
        let id = NEXT_MARKER.fetch_add(1, Ordering::Relaxed);
        profile.markers.push(name.to_owned());
        id
    })
}

/// The marker names registered so far.
#[must_use]
pub fn get_markers() -> Vec<String> {
    with(|profile| profile.markers.clone())
}

/// Registers a trace name and answers its id.
pub fn register_trace(name: &str) -> u64 {
    with(|profile| {
        let id = NEXT_TRACE.fetch_add(1, Ordering::Relaxed);
        profile.traces.push(name.to_owned());
        id
    })
}

/// The trace names registered so far.
#[must_use]
pub fn get_traces() -> Vec<String> {
    with(|profile| profile.traces.clone())
}

/// The metrics registrations, under the profiler's names.
pub fn register_counter(name: &str) -> u64 {
    crate::metrics::register_counter(name)
}

/// See [`register_counter`].
pub fn register_gauge(name: &str) -> u64 {
    crate::metrics::register_gauge(name)
}

/// See [`register_counter`].
pub fn register_histogram(name: &str) -> u64 {
    crate::metrics::register_histogram(name)
}

/// Opens a named section. Pair it with [`end_section`].
pub fn begin_section(name: &str) {
    with(|profile| {
        profile.stack.push(Frame {
            name: name.to_owned(),
            children: Duration::ZERO,
        });
    });
}

/// Closes the innermost section with its elapsed time, which includes its children. Its self
/// time is that total minus the totals of the sections it directly contained. False when no
/// section was open.
pub fn end_section(elapsed: Duration) -> bool {
    with(|profile| {
        let Some(frame) = profile.stack.pop() else {
            crate::warn!("end_section without a matching begin_section; the sample was dropped");
            return false;
        };
        if let Some(parent) = profile.stack.last_mut() {
            parent.children += elapsed;
        }
        if profile.recording() {
            let self_time = elapsed.saturating_sub(frame.children);
            profile.add_section(&frame.name, elapsed, self_time);
        }
        true
    })
}

/// Opens a named span. Spans are recorded flat, for the slowest-event list.
pub fn begin_span(name: &str) {
    with(|profile| profile.span_stack.push(name.to_owned()));
}

/// Closes the innermost span with its elapsed time. False when no span was open.
pub fn end_span(elapsed: Duration) -> bool {
    with(|profile| {
        let Some(name) = profile.span_stack.pop() else {
            crate::warn!("end_span without a matching begin_span; the sample was dropped");
            return false;
        };
        if profile.recording() {
            profile.spans.push((name, elapsed));
        }
        true
    })
}

/// Records one completed section with no children: a shortcut for begin/end around `elapsed`.
pub fn measure(name: &str, elapsed: Duration) {
    with(|profile| {
        if profile.recording() {
            profile.add_section(name, elapsed, elapsed);
        }
    });
}

/// Records one frame's duration into the ring.
pub fn record_frame(duration: Duration) -> usize {
    with(|profile| {
        if profile.frames.len() == FRAME_WINDOW {
            profile.frames.pop_front();
        }
        profile.frames.push_back(duration);
        profile.frames.len()
    })
}

/// Every section's aggregate, in the order they first ran.
#[must_use]
pub fn get_sections() -> Vec<Section> {
    with(|profile| profile.sections.clone())
}

/// One section's aggregate.
#[must_use]
pub fn get_section(name: &str) -> Option<Section> {
    with(|profile| {
        profile
            .sections
            .iter()
            .find(|section| section.name == name)
            .cloned()
    })
}

/// The total recorded across every section, which is what this mod cost.
#[must_use]
pub fn get_mod_cost() -> Duration {
    with(|profile| {
        profile
            .sections
            .iter()
            .fold(Duration::ZERO, |acc, section| acc + section.total)
    })
}

/// The sections ranked by self time, hottest first — the ones actually doing the work.
#[must_use]
pub fn get_hotspots() -> Vec<Section> {
    with(|profile| {
        let mut sections = profile.sections.clone();
        sections.sort_by_key(|section| std::cmp::Reverse(section.self_time));
        sections
    })
}

/// The recorded spans ranked by duration, slowest first.
#[must_use]
pub fn get_slowest_events() -> Vec<(String, Duration)> {
    with(|profile| {
        let mut spans = profile.spans.clone();
        spans.sort_by_key(|span| std::cmp::Reverse(span.1));
        spans
    })
}

/// The frame durations held in the ring, oldest first.
#[must_use]
pub fn get_samples() -> Vec<Duration> {
    with(|profile| profile.frames.iter().copied().collect())
}

/// How many frames the ring holds.
#[must_use]
pub fn sample_count() -> usize {
    with(|profile| profile.frames.len())
}

/// The most recent frame's duration.
#[must_use]
pub fn get_current_frame_time() -> Option<Duration> {
    with(|profile| profile.frames.back().copied())
}

/// The mean frame duration over the ring.
#[must_use]
pub fn get_average() -> Option<Duration> {
    with(|profile| {
        if profile.frames.is_empty() {
            return None;
        }
        let total = profile
            .frames
            .iter()
            .fold(Duration::ZERO, |acc, frame| acc + *frame);
        Some(total / profile.frames.len() as u32)
    })
}

/// The longest frame in the ring.
#[must_use]
pub fn get_maximum() -> Option<Duration> {
    with(|profile| profile.frames.iter().copied().max())
}

/// The shortest frame in the ring.
#[must_use]
pub fn get_minimum() -> Option<Duration> {
    with(|profile| profile.frames.iter().copied().min())
}

/// A frame percentile by nearest rank, the same method [`crate::metrics`] uses.
#[must_use]
pub fn get_percentile(percent: f64) -> Option<Duration> {
    with(|profile| {
        if profile.frames.is_empty() {
            return None;
        }
        let mut sorted: Vec<Duration> = profile.frames.iter().copied().collect();
        sorted.sort();
        let n = sorted.len();
        let rank = ((percent.clamp(0.0, 100.0) / 100.0) * n as f64).ceil() as usize;
        Some(sorted[rank.clamp(1, n) - 1])
    })
}

/// The frames strictly longer than the slow-frame threshold.
#[must_use]
pub fn get_slow_frames() -> Vec<Duration> {
    with(|profile| {
        profile
            .frames
            .iter()
            .copied()
            .filter(|frame| *frame > profile.slow_threshold)
            .collect()
    })
}

/// How many frames were flagged slow.
#[must_use]
pub fn get_slow_frame_count() -> usize {
    get_slow_frames().len()
}

/// The duration above which a frame is flagged slow.
#[must_use]
pub fn get_slow_frame_threshold() -> Duration {
    with(|profile| profile.slow_threshold)
}

/// Sets the slow-frame threshold.
pub fn set_slow_frame_threshold(threshold: Duration) {
    with(|profile| profile.slow_threshold = threshold);
}

/// The profile as JSON, built through [`crate::serialization`].
#[must_use]
pub fn profile_json() -> String {
    with(|profile| {
        let mut sections = Vec::new();
        for section in &profile.sections {
            let mut object = Map::new();
            object.insert("name", Value::Str(section.name.clone()));
            object.insert("calls", Value::Int(section.calls as i64));
            object.insert("total_ms", Value::Float(millis(section.total)));
            object.insert("self_ms", Value::Float(millis(section.self_time)));
            object.insert("min_ms", Value::Float(millis(section.min)));
            object.insert("max_ms", Value::Float(millis(section.max)));
            sections.push(Value::Map(object));
        }
        let mut frames = Map::new();
        frames.insert("count", Value::Int(profile.frames.len() as i64));
        frames.insert(
            "min_ms",
            Value::Float(profile.frames.iter().copied().min().map_or(0.0, millis)),
        );
        frames.insert(
            "max_ms",
            Value::Float(profile.frames.iter().copied().max().map_or(0.0, millis)),
        );
        frames.insert(
            "mean_ms",
            Value::Float(
                if profile.frames.is_empty() {
                    0.0
                } else {
                    millis(
                        profile
                            .frames
                            .iter()
                            .fold(Duration::ZERO, |acc, frame| acc + *frame)
                            / profile.frames.len() as u32,
                    )
                },
            ),
        );
        frames.insert(
            "p50_ms",
            Value::Float(percentile_ms(profile, 50.0)),
        );
        frames.insert("p90_ms", Value::Float(percentile_ms(profile, 90.0)));
        frames.insert("p99_ms", Value::Float(percentile_ms(profile, 99.0)));

        let slow: Vec<Value> = profile
            .frames
            .iter()
            .copied()
            .filter(|frame| *frame > profile.slow_threshold)
            .map(|frame| Value::Float(millis(frame)))
            .collect();

        let refusals: Vec<Value> = refusals()
            .into_iter()
            .map(|(what, why)| {
                let mut object = Map::new();
                object.insert("what", Value::Str(what));
                object.insert("why", Value::Str(why));
                Value::Map(object)
            })
            .collect();

        let mut root = Map::new();
        root.insert("sections", Value::List(sections));
        let cost = profile
            .sections
            .iter()
            .fold(Duration::ZERO, |acc, section| acc + section.total);
        root.insert("mod_cost_ms", Value::Float(millis(cost)));
        root.insert("frames", Value::Map(frames));
        root.insert("slow_frame_ms", Value::Float(millis(profile.slow_threshold)));
        root.insert("slow_frames_ms", Value::List(slow));
        root.insert(
            "markers",
            Value::List(
                profile
                    .markers
                    .iter()
                    .map(|marker| Value::Str(marker.clone()))
                    .collect(),
            ),
        );
        root.insert("refusals", Value::List(refusals));
        serialization::to_json(&Value::Map(root)).unwrap_or_default()
    })
}

fn percentile_ms(profile: &Profile, percent: f64) -> f64 {
    if profile.frames.is_empty() {
        return 0.0;
    }
    let mut sorted: Vec<Duration> = profile.frames.iter().copied().collect();
    sorted.sort();
    let n = sorted.len();
    let rank = ((percent.clamp(0.0, 100.0) / 100.0) * n as f64).ceil() as usize;
    millis(sorted[rank.clamp(1, n) - 1])
}

/// The profile as text.
#[must_use]
pub fn create_report() -> String {
    let mut out = String::new();
    let sections = get_sections();
    out.push_str(&format!("sections={}\n", sections.len()));
    for section in sections {
        out.push_str(&format!(
            "  {} calls={} total={}ms self={}ms min={}ms max={}ms\n",
            section.name,
            section.calls,
            millis(section.total),
            millis(section.self_time),
            millis(section.min),
            millis(section.max),
        ));
    }
    out.push_str(&format!("mod cost {}ms\n", millis(get_mod_cost())));
    if let Some(mean) = get_average() {
        out.push_str(&format!(
            "frames {} mean={}ms p50={}ms p99={}ms max={}ms slow={} (threshold={}ms)\n",
            sample_count(),
            millis(mean),
            get_percentile(50.0).map_or(0.0, millis),
            get_percentile(99.0).map_or(0.0, millis),
            get_maximum().map_or(0.0, millis),
            get_slow_frame_count(),
            millis(get_slow_frame_threshold()),
        ));
    }
    out.push_str(&format!("markers={:?}\n", get_markers()));
    for (what, why) in refusals() {
        out.push_str(&format!("refused {what}: {why}\n"));
    }
    out
}

/// Writes the JSON profile to a file, and answers where it went. With no path it goes into the
/// mod's data directory as `profile.json`.
pub fn export(path: Option<&Path>) -> std::io::Result<PathBuf> {
    let path = match path {
        Some(path) => path.to_owned(),
        None => {
            if let Some(dir) = crate::metadata::data_directory() {
                std::fs::create_dir_all(&dir)?;
                dir.join("profile.json")
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "no directory is known for this mod",
                ));
            }
        }
    };
    std::fs::write(&path, profile_json())?;
    Ok(path)
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn refuse(what: &str) {
    crate::warn!(
        "a mod asked for the client's own profiling: {}",
        why_unknown(what)
    );
}

/// The client's frame rate. `None`: see [`why_unknown`].
#[must_use]
pub fn get_fps() -> Option<f64> {
    refuse("get_fps");
    None
}

/// The client's frame time. `None`: see [`why_unknown`].
#[must_use]
pub fn get_frame_time() -> Option<Duration> {
    refuse("get_frame_time");
    None
}

/// The client's tick time. `None`: see [`why_unknown`].
#[must_use]
pub fn get_tick_time() -> Option<Duration> {
    refuse("get_tick_time");
    None
}

/// The client's memory use. `None`: see [`why_unknown`].
#[must_use]
pub fn get_memory_usage() -> Option<u64> {
    refuse("get_memory_usage");
    None
}

/// The client's allocation count. `None`: see [`why_unknown`].
#[must_use]
pub fn get_allocations() -> Option<u64> {
    refuse("get_allocations");
    None
}

/// The client's collection time. `None`: see [`why_unknown`].
#[must_use]
pub fn get_gc_time() -> Option<Duration> {
    refuse("get_gc_time");
    None
}

/// The client's draw calls. `None`: see [`why_unknown`].
#[must_use]
pub fn get_draw_calls() -> Option<u64> {
    refuse("get_draw_calls");
    None
}

/// The client's GPU time. `None`: see [`why_unknown`].
#[must_use]
pub fn get_gpu_time() -> Option<Duration> {
    refuse("get_gpu_time");
    None
}

/// The client's network time. `None`: see [`why_unknown`].
#[must_use]
pub fn get_network_time() -> Option<Duration> {
    refuse("get_network_time");
    None
}

/// The client's system cost. `None`: see [`why_unknown`].
#[must_use]
pub fn get_system_cost() -> Option<Duration> {
    refuse("get_system_cost");
    None
}

/// The client's per-mod costs. Empty: see [`why_unknown`].
#[must_use]
pub fn get_slowest_mods() -> Vec<(String, Duration)> {
    refuse("get_slowest_mods");
    Vec::new()
}

/// The client's slow tasks. Empty: see [`why_unknown`].
#[must_use]
pub fn get_slowest_tasks() -> Vec<(String, Duration)> {
    refuse("get_slowest_tasks");
    Vec::new()
}

/// Why a question about the client's own profiling has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "get_fps" => {
            "the client's frame rate is measured inside its own frame loop, which this host \
             cannot read"
                .to_owned()
        }
        "get_frame_time" => {
            "the client's frame time belongs to its renderer, which this host cannot read"
                .to_owned()
        }
        "get_tick_time" => {
            "the client's tick time is its own scheduler's, not this host's to report".to_owned()
        }
        "get_memory_usage" => {
            "the client's heap is its own; this host can only read this process".to_owned()
        }
        "get_allocations" => {
            "the client's allocation count lives in its own allocator, out of reach".to_owned()
        }
        "get_gc_time" => {
            "time spent in the client's collector is measured inside it, not here".to_owned()
        }
        "get_draw_calls" => {
            "the client's draw calls are recorded in its renderer, which this host cannot see"
                .to_owned()
        }
        "get_gpu_time" => {
            "the GPU pass is inside the client's renderer, which exposes no timing here"
                .to_owned()
        }
        "get_network_time" => {
            "the client owns its network thread and reports no timing to a mod".to_owned()
        }
        "get_system_cost" => {
            "the client's system cost comes from its own profiler, which a mod cannot read"
                .to_owned()
        }
        "get_slowest_mods" => {
            "per-mod costs come from the client's profiler, which this host cannot read".to_owned()
        }
        "get_slowest_tasks" => {
            "slow tasks come from the client's profiler and scheduler, out of reach".to_owned()
        }
        _ => format!("{what} is not something this host can profile"),
    }
}

/// What this host cannot read about the client's own profiling.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    [
        "get_fps",
        "get_frame_time",
        "get_tick_time",
        "get_memory_usage",
        "get_allocations",
        "get_gc_time",
        "get_draw_calls",
        "get_gpu_time",
        "get_network_time",
        "get_system_cost",
        "get_slowest_mods",
        "get_slowest_tasks",
    ]
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

    #[test]
    fn a_nested_section_excludes_its_child_from_self_time() {
        let _serial = serial();
        reset();
        begin_section("a");
        begin_section("b");
        begin_section("c");
        end_section(Duration::from_millis(4));
        end_section(Duration::from_millis(10));
        end_section(Duration::from_millis(25));

        let a = get_section("a").expect("a");
        assert_eq!(a.total, Duration::from_millis(25));
        assert_eq!(a.self_time, Duration::from_millis(15), "a's child b is excluded");
        assert_eq!(a.calls, 1);
        let b = get_section("b").expect("b");
        assert_eq!(b.total, Duration::from_millis(10));
        assert_eq!(b.self_time, Duration::from_millis(6), "b's child c is excluded");
        let c = get_section("c").expect("c");
        assert_eq!(c.total, Duration::from_millis(4));
        assert_eq!(c.self_time, Duration::from_millis(4), "c has no child");
    }

    #[test]
    fn a_section_accumulates_calls_and_keeps_its_min_and_max() {
        let _serial = serial();
        reset();
        measure("step", Duration::from_millis(10));
        measure("step", Duration::from_millis(20));
        let step = get_section("step").expect("step");
        assert_eq!(step.calls, 2);
        assert_eq!(step.total, Duration::from_millis(30));
        assert_eq!(step.self_time, Duration::from_millis(30));
        assert_eq!(step.min, Duration::from_millis(10));
        assert_eq!(step.max, Duration::from_millis(20));
    }

    #[test]
    fn the_frame_sampler_answers_current_mean_and_extremes() {
        let _serial = serial();
        reset();
        for millis in [10, 20, 30, 40] {
            record_frame(Duration::from_millis(millis));
        }
        assert_eq!(sample_count(), 4);
        assert_eq!(get_current_frame_time(), Some(Duration::from_millis(40)));
        assert_eq!(get_average(), Some(Duration::from_millis(25)));
        assert_eq!(get_maximum(), Some(Duration::from_millis(40)));
        assert_eq!(get_minimum(), Some(Duration::from_millis(10)));
        assert_eq!(get_percentile(50.0), Some(Duration::from_millis(20)));
        assert_eq!(get_percentile(90.0), Some(Duration::from_millis(40)));
        assert_eq!(get_percentile(99.0), Some(Duration::from_millis(40)));
        assert_eq!(get_samples().len(), 4);
    }

    #[test]
    fn a_slow_frame_is_flagged_above_the_threshold() {
        let _serial = serial();
        reset();
        set_slow_frame_threshold(Duration::from_millis(16));
        assert_eq!(get_slow_frame_threshold(), Duration::from_millis(16));
        for millis in [10, 16, 17, 120] {
            record_frame(Duration::from_millis(millis));
        }
        assert_eq!(
            get_slow_frames(),
            vec![Duration::from_millis(17), Duration::from_millis(120)],
            "strictly above the threshold, so 16 is not slow"
        );
        assert_eq!(get_slow_frame_count(), 2);
    }

    #[test]
    fn hotspots_rank_by_self_time_and_mod_cost_sums_the_totals() {
        let _serial = serial();
        reset();
        measure("render", Duration::from_millis(5));
        measure("ai", Duration::from_millis(9));
        measure("render", Duration::from_millis(2));
        assert_eq!(get_mod_cost(), Duration::from_millis(16));
        let hotspots = get_hotspots();
        assert_eq!(hotspots[0].name, "ai", "9ms of self time leads");
        assert_eq!(hotspots[0].self_time, Duration::from_millis(9));
        assert_eq!(hotspots[1].name, "render");
        assert_eq!(hotspots[1].self_time, Duration::from_millis(7));
    }

    #[test]
    fn stop_and_pause_hold_collection_without_breaking_the_stack() {
        let _serial = serial();
        reset();
        stop();
        assert!(!is_running());
        measure("idle", Duration::from_millis(9));
        assert!(get_section("idle").is_none(), "stopped records nothing");
        start();
        measure("live", Duration::from_millis(9));
        assert!(get_section("live").is_some());
        pause();
        assert!(is_paused());
        measure("held", Duration::from_millis(9));
        assert!(get_section("held").is_none(), "paused records nothing");
        resume();
        assert!(!is_paused());
        measure("live", Duration::from_millis(1));
        assert_eq!(get_section("live").expect("live").calls, 2);
    }

    #[test]
    fn the_profile_exports_json_and_a_text_report() {
        let _serial = serial();
        reset();
        measure("tick", Duration::from_millis(10));
        record_frame(Duration::from_millis(20));
        register_marker("world-save");
        let json = profile_json();
        assert!(json.contains(r#""mod_cost_ms":10"#), "{json}");
        assert!(json.contains(r#""name":"tick""#), "{json}");
        assert!(json.contains("world-save"), "{json}");
        assert!(serialization::from_json(&json).is_ok(), "valid JSON");
        assert!(create_report().contains("mod cost 10ms"), "{}", create_report());

        let path = std::env::temp_dir().join("cinnaloader-profile-probe.json");
        let written = export(Some(&path)).expect("the profile is writable");
        let text = std::fs::read_to_string(&written).expect("and readable");
        assert!(text.contains(r#""sections""#), "{text}");
        let _ = std::fs::remove_file(written);
    }

    #[test]
    fn the_clients_timings_and_memory_are_refused_not_faked() {
        let _serial = serial();
        reset();
        assert!(get_fps().is_none());
        assert!(get_frame_time().is_none());
        assert!(get_tick_time().is_none());
        assert!(get_memory_usage().is_none());
        assert!(get_allocations().is_none());
        assert!(get_gc_time().is_none());
        assert!(get_draw_calls().is_none());
        assert!(get_gpu_time().is_none());
        assert!(get_network_time().is_none());
        assert!(get_system_cost().is_none());
        assert!(get_slowest_mods().is_empty());
        assert!(get_slowest_tasks().is_empty());

        let refusals = refusals();
        assert_eq!(refusals.len(), 12);
        for (what, why) in refusals {
            assert_eq!(why_unknown(&what), why);
            assert!(!why.is_empty());
        }
        assert!(why_unknown("get_fps").contains("frame rate"));
        assert!(why_unknown("get_memory_usage").contains("heap"));
        assert!(why_unknown("something_else").contains("something_else"));
    }
}
