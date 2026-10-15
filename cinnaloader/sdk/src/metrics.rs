//! Named measurements a mod keeps: counters, gauges and histograms, with real aggregation.
//!
//! A **counter** is monotonic and only rises; a **gauge** goes up and down; a **histogram**
//! records values into a ring of fixed capacity and reports `count`, `min`, `max`, `mean` and
//! the percentiles `p50`/`p90`/`p99`. The percentiles use the **nearest-rank** method: over the
//! `n` values currently in the window, sorted ascending, a percentile `p` is the value at
//! 1-based rank `ceil(p/100 * n)`. `p50` of an even-sized window is therefore the lower of the
//! middle two, not their mean.
//!
//! A counter also answers a **rate**: [`mark`] records a baseline, [`delta`] is how much it has
//! risen since, and [`rate`] divides that delta by a window the caller states — no wall clock
//! is read, so a caller controls the number exactly.
//!
//! ```
//! use cinnaloader::metrics;
//! use std::time::Duration;
//!
//! metrics::reset();
//! metrics::register_counter("doc.requests");
//! metrics::increment("doc.requests", 5);
//! assert_eq!(
//!     metrics::get("doc.requests").and_then(|metric| metric.counter()),
//!     Some(5)
//! );
//!
//! metrics::register_histogram("doc.frame_ms");
//! for value in [10.0, 20.0, 30.0, 40.0] {
//!     metrics::observe("doc.frame_ms", value);
//! }
//! let stats = metrics::get("doc.frame_ms")
//!     .and_then(|metric| metric.histogram())
//!     .expect("a histogram");
//! assert_eq!(stats.count, 4);
//! assert_eq!(stats.mean, 25.0);
//! assert_eq!(stats.p50, 20.0);
//! assert_eq!(stats.p99, 40.0);
//!
//! metrics::register_counter("doc.bytes");
//! metrics::increment("doc.bytes", 600);
//! metrics::mark("doc.bytes");
//! metrics::increment("doc.bytes", 150);
//! assert_eq!(metrics::rate("doc.bytes", Duration::from_secs(3)), Some(50.0));
//! ```

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::serialization::{self, Map, Value};

/// The default capacity of a histogram's ring.
pub const WINDOW: usize = 64;

/// What a named metric is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Counter,
    Gauge,
    Histogram,
}

impl Kind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Counter => "counter",
            Self::Gauge => "gauge",
            Self::Histogram => "histogram",
        }
    }
}

/// The aggregation of a histogram's window.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
}

/// One metric as a value, for a caller that wants fields rather than a registry lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum Metric {
    Counter { name: String, value: u64 },
    Gauge { name: String, value: i64 },
    Histogram { name: String, stats: Stats },
}

impl Metric {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Counter { name, .. } | Self::Gauge { name, .. } | Self::Histogram { name, .. } => {
                name
            }
        }
    }

    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Self::Counter { .. } => Kind::Counter,
            Self::Gauge { .. } => Kind::Gauge,
            Self::Histogram { .. } => Kind::Histogram,
        }
    }

    #[must_use]
    pub fn counter(&self) -> Option<u64> {
        match self {
            Self::Counter { value, .. } => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn gauge(&self) -> Option<i64> {
        match self {
            Self::Gauge { value, .. } => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn histogram(&self) -> Option<Stats> {
        match self {
            Self::Histogram { stats, .. } => Some(*stats),
            _ => None,
        }
    }
}

enum Held {
    Counter { value: u64, base: u64 },
    Gauge { value: i64 },
    Histogram { ring: VecDeque<f64>, capacity: usize },
}

struct Entry {
    id: u64,
    name: String,
    held: Held,
}

#[derive(Default)]
struct Registry {
    entries: Vec<Entry>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = registry().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn register(name: &str, make: impl FnOnce() -> Held) -> u64 {
    with(|registry| {
        if let Some(entry) = registry.entries.iter().find(|entry| entry.name == name) {
            return entry.id;
        }
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        registry.entries.push(Entry {
            id,
            name: name.to_owned(),
            held: make(),
        });
        id
    })
}

/// Registers a monotonic counter, or answers the existing registration's id.
pub fn register_counter(name: &str) -> u64 {
    register(name, || Held::Counter { value: 0, base: 0 })
}

/// Registers a gauge that moves in both directions.
pub fn register_gauge(name: &str) -> u64 {
    register(name, || Held::Gauge { value: 0 })
}

/// Registers a histogram with the default window.
pub fn register_histogram(name: &str) -> u64 {
    register_histogram_with(name, WINDOW)
}

/// Registers a histogram with a window of `capacity` values.
pub fn register_histogram_with(name: &str, capacity: usize) -> u64 {
    let capacity = capacity.max(1);
    register(name, || Held::Histogram {
        ring: VecDeque::with_capacity(capacity),
        capacity,
    })
}

fn mutate(name: &str, body: impl FnOnce(&mut Held) -> bool) -> bool {
    with(|registry| {
        match registry.entries.iter_mut().find(|entry| entry.name == name) {
            Some(entry) => body(&mut entry.held),
            None => false,
        }
    })
}

/// Adds `by` to a counter or a gauge. False when there is no such metric, or it is a
/// histogram.
pub fn increment(name: &str, by: u64) -> bool {
    mutate(name, |held| match held {
        Held::Counter { value, .. } => {
            *value = value.saturating_add(by);
            true
        }
        Held::Gauge { value } => {
            *value = value.saturating_add(by as i64);
            true
        }
        Held::Histogram { .. } => false,
    })
}

/// Subtracts `by` from a gauge. False for every other kind: a counter is monotonic.
pub fn decrement(name: &str, by: i64) -> bool {
    mutate(name, |held| match held {
        Held::Gauge { value } => {
            *value = value.saturating_sub(by);
            true
        }
        _ => false,
    })
}

/// Sets a gauge. False for every other kind.
pub fn set(name: &str, value: i64) -> bool {
    mutate(name, |held| match held {
        Held::Gauge { value: held } => {
            *held = value;
            true
        }
        _ => false,
    })
}

/// Records one value into a histogram's window. A non-finite value is refused rather than
/// allowed to poison the aggregation.
pub fn observe(name: &str, value: f64) -> bool {
    if !value.is_finite() {
        crate::warn!("a histogram observation of {value} was refused: only finite values are kept");
        return false;
    }
    mutate(name, |held| match held {
        Held::Histogram { ring, capacity } => {
            if ring.len() == *capacity {
                ring.pop_front();
            }
            ring.push_back(value);
            true
        }
        _ => false,
    })
}

/// Records a counter's baseline, so [`delta`] and [`rate`] measure from here. False for a
/// metric that is not a counter.
pub fn mark(name: &str) -> bool {
    mutate(name, |held| match held {
        Held::Counter { value, base } => {
            *base = *value;
            true
        }
        _ => false,
    })
}

/// How much a counter has risen since its last [`mark`]. `None` when there is no such counter.
#[must_use]
pub fn delta(name: &str) -> Option<u64> {
    with(|registry| {
        registry
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .and_then(|entry| match &entry.held {
                Held::Counter { value, base } => Some(value.saturating_sub(*base)),
                _ => None,
            })
    })
}

/// A counter's delta per second over the window the caller states. The window is not measured
/// — it is exactly what the caller passed — so the number is reproducible. `None` for a
/// metric that is not a counter, or a window of zero seconds.
#[must_use]
pub fn rate(name: &str, window: Duration) -> Option<f64> {
    let seconds = window.as_secs_f64();
    if seconds <= 0.0 {
        return None;
    }
    delta(name).map(|delta| delta as f64 / seconds)
}

/// One metric's current value.
#[must_use]
pub fn get(name: &str) -> Option<Metric> {
    with(|registry| {
        registry
            .entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| snapshot_of(&entry.name, &entry.held))
    })
}

/// Every registered metric, in registration order.
#[must_use]
pub fn get_all() -> Vec<Metric> {
    with(|registry| {
        registry
            .entries
            .iter()
            .map(|entry| snapshot_of(&entry.name, &entry.held))
            .collect()
    })
}

/// The same as [`get_all`], under the name the spec uses.
#[must_use]
pub fn snapshot() -> Vec<Metric> {
    get_all()
}

/// Removes a metric; false when there was none.
pub fn remove(name: &str) -> bool {
    with(|registry| {
        let before = registry.entries.len();
        registry.entries.retain(|entry| entry.name != name);
        registry.entries.len() != before
    })
}

/// Removes every metric.
pub fn reset() {
    with(|registry| registry.entries.clear());
}

/// How many metrics are registered.
#[must_use]
pub fn len() -> usize {
    with(|registry| registry.entries.len())
}

/// The registry as text, one metric per line.
#[must_use]
pub fn report() -> String {
    let metrics = get_all();
    if metrics.is_empty() {
        return "no metrics are registered\n".to_owned();
    }
    let mut out = String::new();
    for metric in metrics {
        match metric {
            Metric::Counter { name, value } => {
                out.push_str(&format!("counter {name} = {value}\n"));
            }
            Metric::Gauge { name, value } => {
                out.push_str(&format!("gauge {name} = {value}\n"));
            }
            Metric::Histogram { name, stats } => {
                out.push_str(&format!(
                    "histogram {name} count={} min={} max={} mean={} p50={} p90={} p99={}\n",
                    stats.count,
                    stats.min,
                    stats.max,
                    stats.mean,
                    stats.p50,
                    stats.p90,
                    stats.p99,
                ));
            }
        }
    }
    out
}

/// The registry as JSON, built through [`crate::serialization`].
#[must_use]
pub fn export_json() -> String {
    let mut metrics = Map::new();
    for metric in get_all() {
        let mut object = Map::new();
        match metric {
            Metric::Counter { name, value } => {
                object.insert("kind", Value::Str("counter".to_owned()));
                object.insert("value", Value::Int(value as i64));
                metrics.insert(name, Value::Map(object));
            }
            Metric::Gauge { name, value } => {
                object.insert("kind", Value::Str("gauge".to_owned()));
                object.insert("value", Value::Int(value));
                metrics.insert(name, Value::Map(object));
            }
            Metric::Histogram { name, stats } => {
                object.insert("kind", Value::Str("histogram".to_owned()));
                object.insert("count", Value::Int(stats.count as i64));
                object.insert("min", Value::Float(stats.min));
                object.insert("max", Value::Float(stats.max));
                object.insert("mean", Value::Float(stats.mean));
                object.insert("p50", Value::Float(stats.p50));
                object.insert("p90", Value::Float(stats.p90));
                object.insert("p99", Value::Float(stats.p99));
                metrics.insert(name, Value::Map(object));
            }
        }
    }
    let mut root = Map::new();
    root.insert("metrics", Value::Map(metrics));
    serialization::to_json(&Value::Map(root)).unwrap_or_default()
}

/// The registry as CSV, one metric per row.
#[must_use]
pub fn export_csv() -> String {
    let mut out = String::from("name,kind,value,count,min,max,mean,p50,p90,p99\n");
    for metric in get_all() {
        match metric {
            Metric::Counter { name, value } => {
                out.push_str(&format!("{name},counter,{value},,,,,,,\n"));
            }
            Metric::Gauge { name, value } => {
                out.push_str(&format!("{name},gauge,{value},,,,,,,\n"));
            }
            Metric::Histogram { name, stats } => {
                out.push_str(&format!(
                    "{name},histogram,,{},{},{},{},{},{},{}\n",
                    stats.count, stats.min, stats.max, stats.mean, stats.p50, stats.p90, stats.p99,
                ));
            }
        }
    }
    out
}

fn snapshot_of(name: &str, held: &Held) -> Metric {
    match held {
        Held::Counter { value, .. } => Metric::Counter {
            name: name.to_owned(),
            value: *value,
        },
        Held::Gauge { value } => Metric::Gauge {
            name: name.to_owned(),
            value: *value,
        },
        Held::Histogram { ring, .. } => Metric::Histogram {
            name: name.to_owned(),
            stats: stats(ring),
        },
    }
}

fn norm(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

fn stats(ring: &VecDeque<f64>) -> Stats {
    if ring.is_empty() {
        return Stats::default();
    }
    let mut sorted: Vec<f64> = ring.iter().copied().collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = sorted.len();
    let sum = sorted.iter().fold(0.0_f64, |acc, value| acc + value);
    let mean = sum / count as f64;
    Stats {
        count,
        min: norm(sorted[0]),
        max: norm(sorted[count - 1]),
        mean: norm(mean),
        p50: percentile(&sorted, 50.0),
        p90: percentile(&sorted, 90.0),
        p99: percentile(&sorted, 99.0),
    }
}

fn percentile(sorted: &[f64], percent: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = ((percent.clamp(0.0, 100.0) / 100.0) * n as f64).ceil() as usize;
    norm(sorted[rank.clamp(1, n) - 1])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    #[test]
    fn a_counter_rises_and_only_a_gauge_moves_down() {
        let _serial = serial();
        reset();
        register_counter("probe.requests");
        assert!(increment("probe.requests", 3));
        assert!(increment("probe.requests", 4));
        assert_eq!(
            get("probe.requests").and_then(|metric| metric.counter()),
            Some(7)
        );
        assert!(!decrement("probe.requests", 2), "a counter is monotonic");
        assert!(!set("probe.requests", 0), "and it cannot be set");
        assert_eq!(get("probe.requests").and_then(|metric| metric.counter()), Some(7));
    }

    #[test]
    fn a_gauge_sets_and_moves_in_both_directions() {
        let _serial = serial();
        reset();
        register_gauge("probe.workers");
        assert!(set("probe.workers", 10));
        assert!(increment("probe.workers", 5));
        assert!(decrement("probe.workers", 8));
        assert_eq!(
            get("probe.workers").and_then(|metric| metric.gauge()),
            Some(7)
        );
        assert_eq!(get("probe.workers").map(|metric| metric.kind()), Some(Kind::Gauge));
    }

    #[test]
    fn a_histogram_aggregates_a_known_sample_exactly() {
        let _serial = serial();
        reset();
        register_histogram("probe.frame_ms");
        for value in 1..=10 {
            assert!(observe("probe.frame_ms", f64::from(value)));
        }
        let stats = get("probe.frame_ms")
            .and_then(|metric| metric.histogram())
            .expect("a histogram");
        assert_eq!(stats.count, 10);
        assert_eq!(stats.min, 1.0);
        assert_eq!(stats.max, 10.0);
        assert_eq!(stats.mean, 5.5);
        assert_eq!(stats.p50, 5.0, "ceil(0.5*10) is rank 5");
        assert_eq!(stats.p90, 9.0, "ceil(0.9*10) is rank 9");
        assert_eq!(stats.p99, 10.0, "ceil(0.99*10) rounds up to rank 10");
    }

    #[test]
    fn the_ring_keeps_only_its_capacity_and_the_newest_values() {
        let _serial = serial();
        reset();
        register_histogram_with("probe.ring", 3);
        for value in [1.0, 2.0, 3.0, 4.0, 5.0] {
            observe("probe.ring", value);
        }
        let stats = get("probe.ring")
            .and_then(|metric| metric.histogram())
            .expect("a histogram");
        assert_eq!(stats.count, 3, "the window holds three, not five");
        assert_eq!(stats.min, 3.0, "the oldest two were overwritten");
        assert_eq!(stats.max, 5.0);
        assert_eq!(stats.mean, 4.0);
    }

    #[test]
    fn percentiles_use_nearest_rank_and_clamp_at_the_ends() {
        let _serial = serial();
        reset();
        register_histogram("probe.ends");
        for value in [10.0, 20.0, 30.0, 40.0] {
            observe("probe.ends", value);
        }
        let stats = get("probe.ends")
            .and_then(|metric| metric.histogram())
            .expect("a histogram");
        assert_eq!(stats.p50, 20.0, "lower of the middle two, nearest-rank");
        assert_eq!(stats.p90, 40.0);
        assert_eq!(stats.p99, 40.0);

        reset();
        register_histogram("probe.wide");
        for value in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 100.0] {
            observe("probe.wide", value);
        }
        let stats = get("probe.wide")
            .and_then(|metric| metric.histogram())
            .expect("a histogram");
        assert_eq!(stats.p50, 5.0);
        assert_eq!(stats.p90, 9.0);
        assert_eq!(stats.p99, 100.0);
    }

    #[test]
    fn a_rate_is_a_delta_over_a_window_the_caller_states() {
        let _serial = serial();
        reset();
        register_counter("probe.bytes");
        increment("probe.bytes", 600);
        mark("probe.bytes");
        increment("probe.bytes", 150);
        assert_eq!(delta("probe.bytes"), Some(150));
        assert_eq!(rate("probe.bytes", Duration::from_secs(3)), Some(50.0));
        assert_eq!(rate("probe.bytes", Duration::from_millis(500)), Some(300.0));
        assert_eq!(rate("probe.bytes", Duration::ZERO), None, "a window needs time");
        mark("probe.bytes");
        assert_eq!(delta("probe.bytes"), Some(0), "a mark resets the baseline");
    }

    #[test]
    fn an_empty_histogram_is_all_positive_zero() {
        let _serial = serial();
        reset();
        register_histogram("probe.empty");
        let stats = get("probe.empty")
            .and_then(|metric| metric.histogram())
            .expect("a histogram");
        assert_eq!(stats.count, 0);
        for value in [stats.min, stats.max, stats.mean, stats.p50, stats.p99] {
            assert_eq!(value, 0.0);
            assert!(value.is_sign_positive(), "no negative zero leaks out");
        }
        assert!(!observe("probe.empty", f64::NAN), "a non-finite value is refused");
        assert!(!observe("probe.empty", f64::INFINITY));
    }

    #[test]
    fn remove_and_reset_empty_the_registry() {
        let _serial = serial();
        reset();
        register_counter("probe.a");
        register_gauge("probe.b");
        assert_eq!(len(), 2);
        assert!(remove("probe.a"));
        assert!(!remove("probe.a"));
        assert_eq!(len(), 1);
        reset();
        assert_eq!(len(), 0);
        assert_eq!(get("probe.b"), None);
        assert!(report().starts_with("no metrics"));
    }

    #[test]
    fn the_exports_render_the_values_exactly() {
        let _serial = serial();
        reset();
        register_counter("probe.hits");
        increment("probe.hits", 5);
        register_histogram("probe.ms");
        for value in [10.0, 20.0, 30.0, 40.0] {
            observe("probe.ms", value);
        }

        let json = export_json();
        assert!(
            json.contains(r#""probe.hits":{"kind":"counter","value":5}"#),
            "{json}"
        );
        assert!(json.contains(r#""count":4"#), "{json}");
        assert!(json.contains(r#""p99":40"#), "{json}");
        assert!(serialization::from_json(&json).is_ok(), "valid JSON");

        let csv = export_csv();
        assert!(csv.starts_with("name,kind,value,count,min,max,mean,p50,p90,p99\n"));
        assert!(csv.contains("probe.hits,counter,5,,,,,,,\n"), "{csv}");
        assert!(csv.contains("probe.ms,histogram,,4,10,40,25,20,40,40\n"), "{csv}");
    }
}
