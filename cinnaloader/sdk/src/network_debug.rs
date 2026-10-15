//! Networking diagnostics: a real packet log a mod feeds, and the client's traffic refused.
//!
//! Everything here works on the mod's own sockets — packets a mod hands in with [`record`],
//! over the connections [`crate::network`] really opens — never on the client's connection,
//! which stays inside the client.
//!
//! * A **packet log** is a ring of [`Record`]s (direction, id, byte size, a monotonic
//!   timestamp in milliseconds, an optional label) with a real [`capacity`] that drops the
//!   oldest, [`record`], [`entries`], [`clear`] and [`count`].
//! * **Aggregates** per id ([`get_packet_counts`], [`get_packet_sizes`]): count, total bytes,
//!   mean and a p95 by nearest-rank over the sorted sizes; plus a total byte count and [`rate`]
//!   over a window the caller states. [`get_bandwidth_history`] buckets the log per integer
//!   second and [`peak_bucket`] names the heaviest.
//! * A **filter** ([`Filter`]) by direction, id, minimum size and time range, combinable, each
//!   matching exactly.
//! * A **capture** ([`Capture`]) exports the log as JSON through [`crate::serialization`] and
//!   imports it back equal; [`compare_recordings`] answers the records added and removed.
//! * A **replay** ([`replay`], [`pause_replay`], [`resume_replay`]) feeds a capture into a
//!   handler in recorded order and reports a real "replayed N of M".
//!
//! Capturing the **client's** own packets, connection or latency is refused with its own
//! sentence in [`why_unknown`] and listed by [`refusals`].
//!
//! ```
//! use cinnaloader::network_debug::{self, Direction, Filter};
//!
//! network_debug::reset();
//! network_debug::set_capacity(3);
//! network_debug::start_recording();
//! assert!(network_debug::record(Direction::ToServer, 7, 10, 1000, None));
//! assert!(network_debug::record(Direction::ToClient, 8, 30, 1500, Some("ping")));
//! assert_eq!(network_debug::count(), 2);
//! assert_eq!(network_debug::get_packet_counts(), vec![(7, 1), (8, 1)]);
//!
//! let capture = network_debug::capture();
//! let json = network_debug::save_recording(&capture);
//! assert_eq!(network_debug::load_recording(&json), Some(capture.clone()));
//!
//! let mut seen = Vec::new();
//! let report = network_debug::replay(&capture, &mut |record| seen.push(record.id));
//! assert_eq!(seen, vec![7, 8]);
//! assert_eq!(report.n_of_m(), "replayed 2 of 2");
//!
//! let filter = Filter::new().direction(Direction::ToServer).min_size(5);
//! assert_eq!(network_debug::filter_packets(&filter).len(), 1);
//! ```

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use crate::serialization::{self, Map, Value};
use crate::{network, packets};

pub use crate::packets::Direction;

/// The default size of the packet log's ring.
pub const DEFAULT_CAPACITY: usize = 256;

/// The marker the capture JSON carries, so a foreign document is refused.
pub const CAPTURE_FORMAT: &str = "network_debug.capture";
/// The capture document version this host writes and reads.
pub const CAPTURE_VERSION: i64 = 1;

const MAX_DECODE_ERRORS: usize = 64;
const BUCKET_MILLIS: u64 = 1000;

/// One packet the log recorded: which way it went, its id, its size, when it happened, and
/// the label a mod attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub direction: Direction,
    pub id: u32,
    pub size: u64,
    /// Monotonic milliseconds; the caller supplies it so a capture replays deterministically.
    pub timestamp: u64,
    pub label: Option<String>,
}

impl Record {
    #[must_use]
    pub fn new(direction: Direction, id: u32, size: u64, timestamp: u64) -> Self {
        Self {
            direction,
            id,
            size,
            timestamp,
            label: None,
        }
    }

    #[must_use]
    pub fn label(mut self, label: &str) -> Self {
        self.label = Some(label.to_owned());
        self
    }
}

/// The whole log as one value, the thing that is saved and replayed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Capture {
    pub records: Vec<Record>,
}

impl Capture {
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// What the log holds in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Statistics {
    pub count: u64,
    pub total_bytes: u64,
    pub ids: usize,
}

/// Per-id aggregation: how many, how many bytes, the mean size and the p95 size.
#[derive(Debug, Clone, PartialEq)]
pub struct SizeStats {
    pub id: u32,
    pub count: u64,
    pub total_bytes: u64,
    pub mean: f64,
    pub p95: u64,
}

/// The bytes recorded in one whole second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bucket {
    pub second: u64,
    pub bytes: u64,
    pub packets: u64,
}

/// A combinable filter over the log. An unset field matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub direction: Option<Direction>,
    pub id: Option<u32>,
    pub min_size: Option<u64>,
    pub from: Option<u64>,
    pub to: Option<u64>,
}

impl Filter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    #[must_use]
    pub fn id(mut self, id: u32) -> Self {
        self.id = Some(id);
        self
    }

    #[must_use]
    pub fn min_size(mut self, min_size: u64) -> Self {
        self.min_size = Some(min_size);
        self
    }

    #[must_use]
    pub fn between(mut self, from: u64, to: u64) -> Self {
        self.from = Some(from);
        self.to = Some(to);
        self
    }

    #[must_use]
    pub fn matches(&self, record: &Record) -> bool {
        self.direction.is_none_or(|want| record.direction == want)
            && self.id.is_none_or(|want| record.id == want)
            && self.min_size.is_none_or(|want| record.size >= want)
            && self.from.is_none_or(|want| record.timestamp >= want)
            && self.to.is_none_or(|want| record.timestamp <= want)
    }
}

/// The records one capture has that another does not, each way.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Diff {
    pub added: Vec<Record>,
    pub removed: Vec<Record>,
}

impl Diff {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    #[must_use]
    pub fn added_ids(&self) -> Vec<u32> {
        self.added.iter().map(|record| record.id).collect()
    }

    #[must_use]
    pub fn removed_ids(&self) -> Vec<u32> {
        self.removed.iter().map(|record| record.id).collect()
    }
}

/// What a replay did: how many records it has fed in total, out of how many, the order of the
/// ids it fed on this call, and whether it reached the end.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReplayReport {
    pub replayed: usize,
    pub total: usize,
    pub order: Vec<u32>,
    pub complete: bool,
}

impl ReplayReport {
    #[must_use]
    pub fn n_of_m(&self) -> String {
        format!("replayed {} of {}", self.replayed, self.total)
    }
}

struct State {
    capacity: usize,
    entries: Vec<Record>,
    recording: bool,
    decode_errors: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            capacity: DEFAULT_CAPACITY,
            entries: Vec::new(),
            recording: true,
            decode_errors: Vec::new(),
        }
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock<T>(held: &'static Mutex<T>) -> MutexGuard<'static, T> {
    held.lock().unwrap_or_else(|error| error.into_inner())
}

/// Appends one packet to the log while recording, dropping the oldest if the ring is full.
/// `false` when recording is off.
pub fn record(
    direction: Direction,
    id: u32,
    size: u64,
    timestamp: u64,
    label: Option<&str>,
) -> bool {
    let mut held = lock(state());
    if !held.recording {
        return false;
    }
    if held.entries.len() >= held.capacity {
        held.entries.remove(0);
    }
    held.entries.push(Record {
        direction,
        id,
        size,
        timestamp,
        label: label.map(str::to_owned),
    });
    true
}

/// The whole log, oldest first.
#[must_use]
pub fn entries() -> Vec<Record> {
    lock(state()).entries.clone()
}

/// Empties the log.
pub fn clear() {
    lock(state()).entries.clear();
}

/// How many records the log holds.
#[must_use]
pub fn count() -> usize {
    lock(state()).entries.len()
}

/// How many records the ring keeps before the oldest are dropped.
#[must_use]
pub fn capacity() -> usize {
    lock(state()).capacity
}

/// Sets the ring's capacity, dropping the oldest records to fit, and answers the new capacity.
pub fn set_capacity(capacity: usize) -> usize {
    let capacity = capacity.max(1);
    let mut held = lock(state());
    held.capacity = capacity;
    while held.entries.len() > capacity {
        held.entries.remove(0);
    }
    capacity
}

/// Whether the log is taking records.
#[must_use]
pub fn is_recording() -> bool {
    lock(state()).recording
}

/// Starts taking records, and answers whether it was already.
pub fn start_recording() -> bool {
    let mut held = lock(state());
    std::mem::replace(&mut held.recording, true)
}

/// Stops taking records, and answers whether it was taking them.
pub fn stop_recording() -> bool {
    let mut held = lock(state());
    std::mem::replace(&mut held.recording, false)
}

/// The log as a capture.
#[must_use]
pub fn capture() -> Capture {
    Capture {
        records: entries(),
    }
}

/// The log's total byte count, record count and distinct ids.
#[must_use]
pub fn get_statistics() -> Statistics {
    let held = lock(state());
    let mut ids = BTreeMap::new();
    let mut total_bytes = 0;
    for record in &held.entries {
        total_bytes += record.size;
        ids.insert(record.id, ());
    }
    Statistics {
        count: held.entries.len() as u64,
        total_bytes,
        ids: ids.len(),
    }
}

/// The total bytes the log holds per second over the window the caller states. The window is
/// not measured — it is exactly what the caller passed — so the number is reproducible.
#[must_use]
pub fn rate(window: Duration) -> f64 {
    let seconds = window.as_secs_f64();
    if seconds <= 0.0 {
        return 0.0;
    }
    let total = get_statistics().total_bytes;
    total as f64 / seconds
}

/// How many of each packet id the log holds, in id order.
#[must_use]
pub fn get_packet_counts() -> Vec<(u32, u64)> {
    let held = lock(state());
    let mut counts: BTreeMap<u32, u64> = BTreeMap::new();
    for record in &held.entries {
        *counts.entry(record.id).or_default() += 1;
    }
    counts.into_iter().collect()
}

/// The per-id aggregation of the log, in id order.
#[must_use]
pub fn get_packet_sizes() -> Vec<SizeStats> {
    let held = lock(state());
    let mut sizes: BTreeMap<u32, Vec<u64>> = BTreeMap::new();
    for record in &held.entries {
        sizes.entry(record.id).or_default().push(record.size);
    }
    sizes
        .into_iter()
        .map(|(id, mut values)| {
            let count = values.len() as u64;
            let total_bytes: u64 = values.iter().sum();
            values.sort_unstable();
            SizeStats {
                id,
                count,
                total_bytes,
                mean: norm(total_bytes as f64 / count as f64),
                p95: p95(&values),
            }
        })
        .collect()
}

fn p95(sorted: &[u64]) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let n = sorted.len();
    let rank = ((0.95 * n as f64).ceil() as usize).clamp(1, n);
    sorted[rank - 1]
}

fn norm(value: f64) -> f64 {
    if value == 0.0 {
        0.0
    } else {
        value
    }
}

/// The records matching a filter, oldest first. Its length is the exact matching count.
#[must_use]
pub fn filter_packets(filter: &Filter) -> Vec<Record> {
    lock(state())
        .entries
        .iter()
        .filter(|record| filter.matches(record))
        .cloned()
        .collect()
}

/// The bytes recorded in each whole second of the log's timestamps, in second order.
#[must_use]
pub fn get_bandwidth_history() -> Vec<Bucket> {
    let held = lock(state());
    let mut buckets: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for record in &held.entries {
        let bucket = buckets.entry(record.timestamp / BUCKET_MILLIS).or_default();
        bucket.0 += record.size;
        bucket.1 += 1;
    }
    buckets
        .into_iter()
        .map(|(second, (bytes, packets))| Bucket {
            second,
            bytes,
            packets,
        })
        .collect()
}

/// The heaviest second in the log, earliest on a tie.
#[must_use]
pub fn peak_bucket() -> Option<Bucket> {
    get_bandwidth_history()
        .into_iter()
        .reduce(|heaviest, bucket| {
            if bucket.bytes > heaviest.bytes {
                bucket
            } else {
                heaviest
            }
        })
}

/// Writes a capture as JSON. Key order is `format`, `version`, `records`, and per record
/// `direction`, `id`, `size`, `timestamp`, `label`.
#[must_use]
pub fn save_recording(capture: &Capture) -> String {
    let mut records = Vec::with_capacity(capture.records.len());
    for record in &capture.records {
        let mut object = Map::new();
        object.insert("direction", Value::Str(record.direction.as_str().to_owned()));
        object.insert("id", Value::Int(i64::from(record.id)));
        object.insert("size", Value::Int(record.size as i64));
        object.insert("timestamp", Value::Int(record.timestamp as i64));
        object.insert(
            "label",
            match &record.label {
                Some(label) => Value::Str(label.clone()),
                None => Value::Null,
            },
        );
        records.push(Value::Map(object));
    }
    let mut root = Map::new();
    root.insert("format", Value::Str(CAPTURE_FORMAT.to_owned()));
    root.insert("version", Value::Int(CAPTURE_VERSION));
    root.insert("records", Value::List(records));
    serialization::to_json(&Value::Map(root)).unwrap_or_default()
}

/// Reads back what [`save_recording`] wrote, refusing a foreign format, version or shape.
#[must_use]
pub fn load_recording(json: &str) -> Option<Capture> {
    let value = serialization::from_json(json).ok()?;
    let root = value.as_map()?;
    if root.get("format")?.as_str()? != CAPTURE_FORMAT {
        return None;
    }
    if root.get("version")?.as_int()? != CAPTURE_VERSION {
        return None;
    }
    let mut records = Vec::new();
    for item in root.get("records")?.as_list()? {
        let object = item.as_map()?;
        let direction = Direction::parse(object.get("direction")?.as_str()?)?;
        let id = u32::try_from(object.get("id")?.as_int()?).ok()?;
        let size = u64::try_from(object.get("size")?.as_int()?).ok()?;
        let timestamp = u64::try_from(object.get("timestamp")?.as_int()?).ok()?;
        let label = match object.get("label")? {
            Value::Null => None,
            Value::Str(text) => Some(text.clone()),
            _ => return None,
        };
        records.push(Record {
            direction,
            id,
            size,
            timestamp,
            label,
        });
    }
    Some(Capture { records })
}

/// The records in `from` that `against` does not hold, one for one.
fn missing(from: &[Record], against: &[Record]) -> Vec<Record> {
    let mut pool = against.to_vec();
    let mut out = Vec::new();
    for record in from {
        match pool.iter().position(|held| held == record) {
            Some(at) => {
                pool.remove(at);
            }
            None => out.push(record.clone()),
        }
    }
    out
}

/// What changed between two captures: the records added and the records removed, each with
/// its ids available through [`Diff::added_ids`] and [`Diff::removed_ids`].
#[must_use]
pub fn compare_recordings(before: &Capture, after: &Capture) -> Diff {
    Diff {
        added: missing(&after.records, &before.records),
        removed: missing(&before.records, &after.records),
    }
}

struct Replay {
    records: Vec<Record>,
    position: usize,
    paused: bool,
    speed: f64,
}

impl Default for Replay {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            position: 0,
            paused: false,
            speed: 1.0,
        }
    }
}

fn replay_state() -> &'static Mutex<Replay> {
    static REPLAY: OnceLock<Mutex<Replay>> = OnceLock::new();
    REPLAY.get_or_init(|| Mutex::new(Replay::default()))
}

fn advance(handler: &mut dyn FnMut(&Record)) -> ReplayReport {
    let mut order = Vec::new();
    loop {
        // the handler may pause the replay, so the lock is never held across the callback
        let next = {
            let held = lock(replay_state());
            if held.paused || held.position >= held.records.len() {
                None
            } else {
                Some(held.records[held.position].clone())
            }
        };
        let Some(record) = next else {
            break;
        };
        handler(&record);
        order.push(record.id);
        let mut held = lock(replay_state());
        held.position += 1;
    }
    let held = lock(replay_state());
    ReplayReport {
        replayed: held.position,
        total: held.records.len(),
        order,
        complete: held.position >= held.records.len(),
    }
}

/// Feeds a capture into a handler, in recorded order, from the start. A handler that calls
/// [`pause_replay`] stops it; [`resume_replay`] carries on from where it stopped.
pub fn replay(capture: &Capture, handler: &mut dyn FnMut(&Record)) -> ReplayReport {
    {
        let mut held = lock(replay_state());
        held.records = capture.records.clone();
        held.position = 0;
        held.paused = false;
    }
    advance(handler)
}

/// Pauses the current replay, and answers whether it was already paused.
pub fn pause_replay() -> bool {
    let mut held = lock(replay_state());
    std::mem::replace(&mut held.paused, true)
}

/// Carries on a paused replay from where it stopped, feeding the rest to the handler.
pub fn resume_replay(handler: &mut dyn FnMut(&Record)) -> ReplayReport {
    lock(replay_state()).paused = false;
    advance(handler)
}

/// How many records the current replay has fed in total.
#[must_use]
pub fn replay_position() -> usize {
    lock(replay_state()).position
}

/// Sets the playback speed, a positive finite factor. `false` refuses anything else.
pub fn set_replay_speed(speed: f64) -> bool {
    if !speed.is_finite() || speed <= 0.0 {
        return false;
    }
    lock(replay_state()).speed = speed;
    true
}

/// The playback speed the replay would run at.
#[must_use]
pub fn replay_speed() -> f64 {
    lock(replay_state()).speed
}

/// The wall-clock span the loaded capture covers at the current speed. `None` when fewer than
/// two records are loaded.
#[must_use]
pub fn replay_duration() -> Option<Duration> {
    let held = lock(replay_state());
    let first = held.records.first()?.timestamp;
    let last = held.records.last()?.timestamp;
    let millis = (last.saturating_sub(first)) as f64 / held.speed;
    if !millis.is_finite() || millis < 0.0 {
        return None;
    }
    Some(Duration::from_millis(millis as u64))
}

/// Takes the length prefix off a frame with [`crate::packets`], recording a failure for
/// [`get_decode_errors`] rather than losing it.
pub fn decode_packet(frame: &[u8]) -> Result<Vec<u8>, packets::PacketError> {
    match packets::decode(frame) {
        Ok(payload) => Ok(payload),
        Err(error) => {
            let mut held = lock(state());
            if held.decode_errors.len() >= MAX_DECODE_ERRORS {
                held.decode_errors.remove(0);
            }
            held.decode_errors.push(error.to_string());
            Err(error)
        }
    }
}

/// The decode failures [`decode_packet`] has seen, oldest first.
#[must_use]
pub fn get_decode_errors() -> Vec<String> {
    lock(state()).decode_errors.clone()
}

/// One line describing a record, for a log or a panel.
#[must_use]
pub fn inspect_packet(record: &Record) -> String {
    let label = record.label.as_deref().unwrap_or("-");
    format!(
        "packet {} {} {} bytes at {} label {}",
        record.id,
        record.direction.as_str(),
        record.size,
        record.timestamp,
        label
    )
}

/// The connections this mod really owns, through [`crate::network`].
#[must_use]
pub fn get_connections() -> Vec<network::Connection> {
    network::connections()
}

/// The diagnostic events this host can fire from the packet log.
pub const PROTOCOL_EVENTS: [&str; 8] = [
    "packet_logged",
    "capture_saved",
    "capture_loaded",
    "replay_started",
    "replay_paused",
    "replay_resumed",
    "replay_completed",
    "decode_error",
];

/// Every diagnostic event kind this module defines.
#[must_use]
pub fn get_protocol_events() -> Vec<&'static str> {
    PROTOCOL_EVENTS.to_vec()
}

/// Whether a name is one of this module's diagnostic event kinds.
#[must_use]
pub fn has_protocol_event(kind: &str) -> bool {
    PROTOCOL_EVENTS.contains(&kind)
}

/// Teaches the event bus each diagnostic event name, answering how many were new.
pub fn register_event_kinds() -> usize {
    PROTOCOL_EVENTS
        .iter()
        .filter(|name| crate::events::register_event_type(name))
        .count()
}

/// Why a question about the client's own traffic has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "packets" => {
            "the client's packets move inside its own connection, and this host sees none of \
             that traffic"
                .to_owned()
        }
        "connection" => {
            "the game holds one connection to its server inside the client, and this host has \
             no handle on it"
                .to_owned()
        }
        "latency" => {
            "the client measures its own latency packet by packet, inside its connection"
                .to_owned()
        }
        "jitter" => "jitter is the client's; this host sees none of its packets".to_owned(),
        "loss" => "packet loss belongs to the client's connection, counted inside it".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["packets", "connection", "latency", "jitter", "loss"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

fn refused<T: Default>(what: &str) -> T {
    crate::warn!(
        "a mod asked this host to capture the client's own traffic: {}",
        why_unknown(what)
    );
    T::default()
}

/// The client's packets. Empty, with [`why_unknown`].
#[must_use]
pub fn get_client_packets() -> Vec<Record> {
    refused("packets")
}

/// The client's connection. `None`, with [`why_unknown`].
#[must_use]
pub fn get_client_connection() -> Option<network::Connection> {
    refused("connection")
}

/// The client's latency, in milliseconds. `None`.
#[must_use]
pub fn get_client_latency() -> Option<u32> {
    refused("latency")
}

/// The client's latency history. Empty.
#[must_use]
pub fn get_latency_history() -> Vec<u32> {
    refused("latency")
}

/// The client's jitter history. Empty.
#[must_use]
pub fn get_jitter_history() -> Vec<u32> {
    refused("jitter")
}

/// The client's packet loss history. Empty.
#[must_use]
pub fn get_loss_history() -> Vec<f64> {
    refused("loss")
}

/// Forgets the whole log, its decode errors, the replay and the recording flag. Tests and a
/// doctest use it to start from the same place.
pub fn reset() {
    let mut held = lock(state());
    held.capacity = DEFAULT_CAPACITY;
    held.entries.clear();
    held.recording = true;
    held.decode_errors.clear();
    *lock(replay_state()) = Replay::default();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn sample() {
        reset();
        set_capacity(64);
        start_recording();
        record(Direction::ToServer, 7, 10, 1000, None);
        record(Direction::ToServer, 7, 20, 1100, Some("keepalive"));
        record(Direction::ToServer, 7, 30, 2000, None);
        record(Direction::ToClient, 8, 200, 2000, None);
        record(Direction::ToServer, 8, 40, 2300, None);
    }

    #[test]
    fn the_ring_keeps_its_capacity_and_drops_the_oldest() {
        let _serial = serial();
        reset();
        set_capacity(3);
        for (index, size) in [1u64, 2, 3, 4, 5].into_iter().enumerate() {
            record(Direction::ToServer, 7, size, 1000 + index as u64, None);
        }
        assert_eq!(count(), 3, "the ring holds three, not five");
        assert_eq!(
            entries().iter().map(|record| record.size).collect::<Vec<_>>(),
            vec![3, 4, 5],
            "the oldest two were dropped"
        );
        clear();
        assert_eq!(count(), 0);
    }

    #[test]
    fn aggregates_report_exact_counts_bytes_mean_and_p95() {
        let _serial = serial();
        sample();
        assert_eq!(get_packet_counts(), vec![(7, 3), (8, 2)]);
        let sizes = get_packet_sizes();
        assert_eq!(sizes.len(), 2);
        assert_eq!(
            sizes[0],
            SizeStats {
                id: 7,
                count: 3,
                total_bytes: 60,
                mean: 20.0,
                p95: 30,
            },
            "ceil(0.95*3) is rank 3"
        );
        assert_eq!(
            sizes[1],
            SizeStats {
                id: 8,
                count: 2,
                total_bytes: 240,
                mean: 120.0,
                p95: 200,
            },
            "ceil(0.95*2) is rank 2"
        );
        let stats = get_statistics();
        assert_eq!(stats.count, 5);
        assert_eq!(stats.total_bytes, 300);
        assert_eq!(stats.ids, 2);
        assert_eq!(rate(Duration::from_secs(2)), 150.0);
        assert_eq!(rate(Duration::ZERO), 0.0, "a window needs time");
        assert!(rate(Duration::from_secs(2)).is_sign_positive());
    }

    #[test]
    fn a_filter_combines_direction_id_size_and_time() {
        let _serial = serial();
        sample();
        assert_eq!(
            filter_packets(&Filter::new().direction(Direction::ToServer)).len(),
            4
        );
        assert_eq!(filter_packets(&Filter::new().id(7)).len(), 3);
        assert_eq!(filter_packets(&Filter::new().min_size(20)).len(), 4);
        assert_eq!(filter_packets(&Filter::new().between(1100, 2000)).len(), 3);
        assert_eq!(filter_packets(&Filter::new()).len(), 5, "an empty filter matches all");

        let composed = Filter::new()
            .direction(Direction::ToServer)
            .id(7)
            .min_size(15)
            .between(1100, 2000);
        let matched = filter_packets(&composed);
        assert_eq!(matched.len(), 2);
        assert_eq!(
            matched.iter().map(|record| record.size).collect::<Vec<_>>(),
            vec![20, 30]
        );
        assert_eq!(filter_packets(&Filter::new().id(7).min_size(25)).len(), 1);
    }

    #[test]
    fn a_capture_round_trips_through_json_exactly() {
        let _serial = serial();
        sample();
        let capture = capture();
        let json = save_recording(&capture);
        assert!(json.starts_with(r#"{"format":"network_debug.capture","version":1,"records":["#));
        assert!(
            json.contains(r#"{"direction":"to_server","id":7,"size":20,"timestamp":1100,"label":"keepalive"}"#),
            "{json}"
        );
        assert!(json.contains(r#""label":null"#), "an unlabelled record round trips");
        assert_eq!(load_recording(&json), Some(capture.clone()));
        assert_eq!(load_recording("{}"), None);
        assert_eq!(load_recording("not json"), None);
        assert_eq!(
            load_recording(r#"{"format":"other","version":1,"records":[]}"#),
            None
        );
    }

    #[test]
    fn a_diff_names_the_records_added_and_removed() {
        let _serial = serial();
        sample();
        let before = capture();
        let removed = before.records[2].clone();
        let mut after = Capture::default();
        for (index, record) in before.records.iter().enumerate() {
            if index != 2 {
                after.records.push(record.clone());
            }
        }
        after
            .records
            .push(Record::new(Direction::ToClient, 9, 12, 3000));

        let diff = compare_recordings(&before, &after);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed_ids(), vec![removed.id]);
        assert_eq!(diff.removed[0], removed);
        assert_eq!(diff.added_ids(), vec![9]);
        assert!(!diff.is_empty());
        assert!(compare_recordings(&before, &before).is_empty());
    }

    #[test]
    fn a_replay_feeds_records_in_recorded_order_and_reports_the_count() {
        let _serial = serial();
        sample();
        let capture = capture();
        let mut seen = Vec::new();
        let report = replay(&capture, &mut |record| seen.push(record.id));
        assert_eq!(seen, vec![7, 7, 7, 8, 8], "the exact recorded order");
        assert_eq!(report.order, seen);
        assert_eq!(report.replayed, 5);
        assert_eq!(report.total, 5);
        assert!(report.complete);
        assert_eq!(report.n_of_m(), "replayed 5 of 5");
    }

    #[test]
    fn a_paused_replay_resumes_where_it_stopped() {
        let _serial = serial();
        sample();
        let capture = capture();
        let mut seen = Vec::new();
        let first = replay(&capture, &mut |record| {
            seen.push(record.id);
            if seen.len() == 3 {
                pause_replay();
            }
        });
        assert_eq!(first.replayed, 3, "the handler paused after the third");
        assert!(!first.complete);
        assert_eq!(first.n_of_m(), "replayed 3 of 5");
        assert_eq!(seen, vec![7, 7, 7]);

        let rest = resume_replay(&mut |record| seen.push(record.id));
        assert_eq!(rest.replayed, 5, "the position is the whole run");
        assert_eq!(rest.order, vec![8, 8], "only the tail was fed this time");
        assert!(rest.complete);
        assert_eq!(rest.n_of_m(), "replayed 5 of 5");
        assert_eq!(seen, vec![7, 7, 7, 8, 8]);
        assert_eq!(replay_position(), 5);

        assert!(set_replay_speed(2.0));
        assert!(!set_replay_speed(0.0));
        assert!(!set_replay_speed(f64::NAN));
        assert_eq!(replay_speed(), 2.0);
        assert_eq!(replay_duration(), Some(Duration::from_millis(650)));
    }

    #[test]
    fn bandwidth_buckets_bytes_per_second_and_finds_the_peak() {
        let _serial = serial();
        sample();
        let buckets = get_bandwidth_history();
        assert_eq!(
            buckets,
            vec![
                Bucket {
                    second: 1,
                    bytes: 30,
                    packets: 2,
                },
                Bucket {
                    second: 2,
                    bytes: 270,
                    packets: 3,
                },
            ]
        );
        assert_eq!(
            peak_bucket(),
            Some(Bucket {
                second: 2,
                bytes: 270,
                packets: 3,
            })
        );
        clear();
        assert!(get_bandwidth_history().is_empty());
        assert_eq!(peak_bucket(), None);
    }

    #[test]
    fn the_clients_traffic_is_refused_and_decode_errors_are_kept() {
        let _serial = serial();
        sample();
        assert!(get_client_packets().is_empty());
        assert!(get_client_connection().is_none());
        assert!(get_client_latency().is_none());
        assert!(get_latency_history().is_empty());
        assert!(get_jitter_history().is_empty());
        assert!(get_loss_history().is_empty());

        assert_eq!(refusals().len(), 5);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }

        assert_eq!(get_decode_errors(), Vec::<String>::new());
        let frame = packets::encode(b"hello");
        assert_eq!(decode_packet(&frame), Ok(b"hello".to_vec()));
        assert!(decode_packet(&frame[..3]).is_err());
        assert_eq!(get_decode_errors().len(), 1, "the failure was kept");
        assert!(
            get_decode_errors()[0].contains("frame declares"),
            "{}",
            get_decode_errors()[0]
        );

        let record = Record::new(Direction::ToServer, 7, 10, 1000).label("ping");
        assert_eq!(inspect_packet(&record), "packet 7 to_server 10 bytes at 1000 label ping");
        assert!(has_protocol_event("packet_logged"));
        assert!(!has_protocol_event("nope"));
        assert_eq!(get_protocol_events().len(), 8);
    }
}
