//! Memory management: an allocation ledger, arenas, slabs and pools a mod owns, and the
//! client's own heap and collector refused.
//!
//! Nothing here allocates on a mod's behalf behind its back: a mod's Rust allocations are
//! made by the global allocator, in the usual way, and every structure here is a store the
//! mod chooses to use. [`Arena`] bumps offsets through a block the mod owns, [`Slab`] keeps a
//! first-fit free list over one, [`Pool`] hands out fixed-size slots, and the ledger records
//! what a mod declared with [`track`] and released with [`untrack`]. [`get_usage`] reports
//! what the mod itself declared, plus this process's resident and virtual size as the
//! operating system tells it; it does not see the client's allocator.
//!
//! The client's own heap, its collector, forcing a collection and reading its memory usage
//! are refused, each with a sentence in [`why_unknown`] and listed by [`refusals`].
//!
//! Fragmentation is real and measured: the arena never reclaims a block until [`Arena::reset`]
//! takes the whole one back, and the slab coalesces a freed block with a free neighbour yet
//! still fails a request that no single free run can hold, however much free space is left.
//!
//! ```
//! use cinnaloader::memory;
//!
//! let mut arena = memory::Arena::new(64);
//! assert_eq!(arena.alloc(16, 8).expect("16 bytes aligned to 8"), 0);
//! assert_eq!(arena.alloc(1, 1).expect("one byte"), 16);
//! assert_eq!(arena.alloc(8, 16).expect("aligned up to 16"), 32);
//! assert!(arena.alloc(64, 1).is_err(), "no room, and no wrap");
//!
//! let mut slab = memory::Slab::new(64);
//! let first = slab.alloc(16, 8).expect("a block");
//! let second = slab.alloc(16, 8).expect("a second block");
//! assert_eq!((first, second), (0, 16));
//! slab.free(first).expect("a live block");
//! assert_eq!(slab.alloc(16, 8).expect("fits again"), first);
//!
//! memory::register_allocation_tracker("hello", Some(1024));
//! let id = memory::track("mesh", 256, "render").expect("within budget");
//! assert!(memory::track("atlas", 4096, "render").is_err(), "past the budget");
//! assert_eq!(memory::get_allocated(), 256);
//! assert!(memory::untrack(id));
//! assert_eq!(memory::get_allocated(), 0);
//!
//! let mut pool = memory::Pool::new(8, 2);
//! let slot = pool.acquire().expect("a free slot");
//! assert_eq!(pool.live_count(), 1);
//! pool.release(slot).expect("a live slot");
//! assert_eq!(pool.live_count(), 0);
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// What went wrong in an arena or a slab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocError {
    /// An alignment that is not a power of two.
    BadAlign(usize),
    /// A request larger than the room left, or larger than any single free run.
    OutOfSpace {
        requested: usize,
        available: usize,
    },
    /// A block that is not one this allocator handed out.
    UnknownOffset(usize),
    /// No arena or slab is registered under that name.
    UnknownBuffer(String),
}

impl fmt::Display for AllocError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadAlign(align) => write!(formatter, "{align} is not a power-of-two alignment"),
            Self::OutOfSpace {
                requested,
                available,
            } => write!(formatter, "{requested} bytes asked, {available} available"),
            Self::UnknownOffset(offset) => write!(formatter, "no block begins at offset {offset}"),
            Self::UnknownBuffer(name) => write!(formatter, "no arena or slab named {name}"),
        }
    }
}

impl std::error::Error for AllocError {}

/// What went wrong in a pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    /// Every slot is taken.
    Exhausted { capacity: usize },
    /// A slot that is not currently held.
    UnknownSlot(usize),
    /// No pool is registered under that name.
    UnknownPool(String),
}

impl fmt::Display for PoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exhausted { capacity } => write!(formatter, "all {capacity} slots are taken"),
            Self::UnknownSlot(slot) => write!(formatter, "slot {slot} is not held"),
            Self::UnknownPool(name) => write!(formatter, "no pool named {name}"),
        }
    }
}

impl std::error::Error for PoolError {}

/// What went wrong in the ledger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrackError {
    /// The declaration would push the tracker past its budget.
    BudgetExceeded {
        requested: u64,
        current: u64,
        budget: u64,
    },
    /// The named tracker is not registered.
    UnknownTracker(String),
    /// A tracker is already registered under that name.
    DuplicateTracker(String),
}

impl fmt::Display for TrackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetExceeded {
                requested,
                current,
                budget,
            } => write!(
                formatter,
                "{requested} more bytes over {current} would pass the {budget}-byte budget"
            ),
            Self::UnknownTracker(name) => write!(formatter, "no allocation tracker named {name}"),
            Self::DuplicateTracker(name) => write!(formatter, "a tracker named {name} is registered"),
        }
    }
}

impl std::error::Error for TrackError {}

fn align_up(value: usize, align: usize) -> Option<usize> {
    let mask = align.checked_sub(1)?;
    value.checked_add(mask).map(|shifted| shifted & !mask)
}

/// A bump allocator over a block the mod owns: offsets only move forward, and nothing is
/// reclaimed until [`Arena::reset`] takes the whole block back.
#[derive(Debug, Clone)]
pub struct Arena {
    block: Vec<u8>,
    next: usize,
}

impl Arena {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            block: vec![0; capacity],
            next: 0,
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.block.len()
    }

    #[must_use]
    pub fn used(&self) -> usize {
        self.next
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.block.len().saturating_sub(self.next)
    }

    /// Answers the offset of `size` bytes aligned to `align`, moving the cursor past them.
    pub fn alloc(&mut self, size: usize, align: usize) -> Result<usize, AllocError> {
        if !align.is_power_of_two() {
            return Err(AllocError::BadAlign(align));
        }
        let Some(start) = align_up(self.next, align) else {
            return Err(AllocError::OutOfSpace {
                requested: size,
                available: self.remaining(),
            });
        };
        let Some(end) = start.checked_add(size) else {
            return Err(AllocError::OutOfSpace {
                requested: size,
                available: self.remaining(),
            });
        };
        if end > self.block.len() {
            return Err(AllocError::OutOfSpace {
                requested: size,
                available: self.block.len().saturating_sub(start),
            });
        }
        self.next = end;
        Ok(start)
    }

    /// Gives the whole block back, and answers how many bytes had been handed out.
    pub fn reset(&mut self) -> usize {
        let used = self.next;
        self.next = 0;
        used
    }

    #[must_use]
    pub fn block(&self) -> &[u8] {
        &self.block
    }

    pub fn block_mut(&mut self) -> &mut [u8] {
        &mut self.block
    }
}

#[derive(Debug, Clone, Copy)]
struct Run {
    offset: usize,
    size: usize,
}

/// A first-fit free-list allocator over a block the mod owns: a freed block goes back on the
/// list, is reused at its own offset when a later request fits, and is coalesced with a free
/// neighbour.
#[derive(Debug, Clone)]
pub struct Slab {
    block: Vec<u8>,
    free: Vec<Run>,
    live: BTreeMap<usize, usize>,
}

impl Slab {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let block = vec![0; capacity];
        let free = match capacity {
            0 => Vec::new(),
            _ => vec![Run {
                offset: 0,
                size: capacity,
            }],
        };
        Self {
            block,
            free,
            live: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.block.len()
    }

    #[must_use]
    pub fn used(&self) -> usize {
        self.live.values().sum()
    }

    #[must_use]
    pub fn free_bytes(&self) -> usize {
        self.free.iter().map(|run| run.size).sum()
    }

    #[must_use]
    pub fn fragments(&self) -> usize {
        self.free.len()
    }

    pub fn alloc(&mut self, size: usize, align: usize) -> Result<usize, AllocError> {
        if !align.is_power_of_two() {
            return Err(AllocError::BadAlign(align));
        }
        let candidate = self.free.iter().position(|run| {
            let Some(start) = align_up(run.offset, align) else {
                return false;
            };
            start
                .checked_add(size)
                .is_some_and(|stop| stop <= run.offset + run.size)
        });
        let Some(index) = candidate else {
            return Err(AllocError::OutOfSpace {
                requested: size,
                available: self.free_bytes(),
            });
        };
        let run = self.free.remove(index);
        let start = align_up(run.offset, align).expect("the candidate aligned");
        let end = run.offset + run.size;
        if start > run.offset {
            self.free.push(Run {
                offset: run.offset,
                size: start - run.offset,
            });
        }
        if start + size < end {
            self.free.push(Run {
                offset: start + size,
                size: end - (start + size),
            });
        }
        self.free.sort_by_key(|run| run.offset);
        self.live.insert(start, size);
        Ok(start)
    }

    pub fn free(&mut self, offset: usize) -> Result<usize, AllocError> {
        let Some(size) = self.live.remove(&offset) else {
            return Err(AllocError::UnknownOffset(offset));
        };
        self.free.push(Run { offset, size });
        self.free.sort_by_key(|run| run.offset);
        self.coalesce();
        Ok(size)
    }

    fn coalesce(&mut self) {
        let mut merged: Vec<Run> = Vec::with_capacity(self.free.len());
        for run in self.free.drain(..) {
            match merged.last_mut() {
                Some(last) if last.offset + last.size == run.offset => last.size += run.size,
                _ => merged.push(run),
            }
        }
        self.free = merged;
    }

    #[must_use]
    pub fn block(&self) -> &[u8] {
        &self.block
    }

    pub fn block_mut(&mut self) -> &mut [u8] {
        &mut self.block
    }
}

/// A fixed-size object pool: slots a mod acquires and releases, reusing a released slot.
#[derive(Debug, Clone)]
pub struct Pool {
    block: Vec<u8>,
    block_size: usize,
    slots: usize,
    free: Vec<usize>,
    live: BTreeSet<usize>,
    acquired: u64,
}

impl Pool {
    #[must_use]
    pub fn new(block_size: usize, slots: usize) -> Self {
        Self {
            block: vec![0; block_size.saturating_mul(slots)],
            block_size,
            slots,
            free: (0..slots).rev().collect(),
            live: BTreeSet::new(),
            acquired: 0,
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slots
    }

    #[must_use]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    #[must_use]
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    #[must_use]
    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    #[must_use]
    pub fn acquired_total(&self) -> u64 {
        self.acquired
    }

    #[must_use]
    pub fn reserved_bytes(&self) -> usize {
        self.block.len()
    }

    pub fn acquire(&mut self) -> Result<usize, PoolError> {
        match self.free.pop() {
            Some(slot) => {
                self.live.insert(slot);
                self.acquired += 1;
                Ok(slot)
            }
            None => Err(PoolError::Exhausted {
                capacity: self.slots,
            }),
        }
    }

    pub fn release(&mut self, slot: usize) -> Result<(), PoolError> {
        if !self.live.remove(&slot) {
            return Err(PoolError::UnknownSlot(slot));
        }
        self.free.push(slot);
        Ok(())
    }

    #[must_use]
    pub fn offset_of(&self, slot: usize) -> Option<usize> {
        (slot < self.slots).then(|| slot * self.block_size)
    }

    #[must_use]
    pub fn block(&self) -> &[u8] {
        &self.block
    }

    pub fn block_mut(&mut self) -> &mut [u8] {
        &mut self.block
    }
}

/// How one pool stands, for a report.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PoolStats {
    pub name: String,
    pub capacity: usize,
    pub block_size: usize,
    pub live: usize,
    pub free: usize,
    pub acquired_total: u64,
    pub reserved_bytes: usize,
}

/// How one arena stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArenaInfo {
    pub name: String,
    pub capacity: usize,
    pub used: usize,
    pub remaining: usize,
}

/// How one slab stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SlabInfo {
    pub name: String,
    pub capacity: usize,
    pub used: usize,
    pub free: usize,
    pub fragments: usize,
}

/// Every heap a mod owns, as one answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeapInfo {
    pub arenas: Vec<ArenaInfo>,
    pub slabs: Vec<SlabInfo>,
    pub pools: Vec<PoolStats>,
    pub reserved_bytes: u64,
}

/// This process and the mod's own ledger, as one answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Usage {
    pub tracked_bytes: u64,
    pub peak_bytes: u64,
    pub allocations: usize,
    pub reserved_bytes: u64,
    pub process_resident_bytes: Option<u64>,
    pub process_virtual_bytes: Option<u64>,
}

/// One allocation the ledger still holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leak {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub owner: String,
}

/// What one owner holds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnerBytes {
    pub owner: String,
    pub bytes: u64,
    pub allocations: usize,
}

/// A moment in the ledger's life.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub tracker: String,
    pub current: u64,
    pub peak: u64,
    pub total: u64,
    pub allocations: usize,
    pub owners: BTreeMap<String, u64>,
}

/// One owner's move between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnerDelta {
    pub owner: String,
    pub before: u64,
    pub after: u64,
    pub delta: i64,
}

/// What changed between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SnapshotDiff {
    pub current_delta: i64,
    pub peak_delta: i64,
    pub total_delta: i64,
    pub allocations_delta: i64,
    pub owners: Vec<OwnerDelta>,
}

/// A threshold the mod watches its own ledger against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PressureEvent {
    pub handler: String,
    pub threshold: u64,
    pub current: u64,
}

/// A named threshold a mod registered.
#[derive(Debug, Clone)]
struct PressureHandler {
    name: String,
    threshold: u64,
}

#[derive(Debug, Clone, Default)]
struct Tracked {
    name: String,
    size: u64,
    owner: String,
}

#[derive(Debug, Clone, Default)]
struct Ledger {
    budget: Option<u64>,
    entries: BTreeMap<u64, Tracked>,
    next_id: u64,
    current: u64,
    peak: u64,
    total: u64,
}

#[derive(Default)]
struct State {
    trackers: BTreeMap<String, Ledger>,
    current: Option<String>,
    arenas: BTreeMap<String, Arena>,
    slabs: BTreeMap<String, Slab>,
    pools: BTreeMap<String, Pool>,
    handlers: Vec<PressureHandler>,
    events: Vec<PressureEvent>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn with_state<T>(body: impl FnOnce(&mut State) -> T) -> T {
    let mut guard: MutexGuard<'_, State> = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

fn ensure_current(state: &mut State) -> &mut Ledger {
    let name = state
        .current
        .get_or_insert_with(|| "default".to_owned())
        .clone();
    state.trackers.entry(name.clone()).or_default();
    state.trackers.get_mut(&name).expect("just inserted")
}

fn current_ledger(state: &State) -> Option<&Ledger> {
    state.trackers.get(state.current.as_deref()?)
}

fn reserved_bytes(state: &State) -> u64 {
    let arenas: usize = state.arenas.values().map(Arena::capacity).sum();
    let slabs: usize = state.slabs.values().map(Slab::capacity).sum();
    let pools: usize = state.pools.values().map(Pool::reserved_bytes).sum();
    (arenas + slabs + pools) as u64
}

fn pool_stats(name: &str, pool: &Pool) -> PoolStats {
    PoolStats {
        name: name.to_owned(),
        capacity: pool.capacity(),
        block_size: pool.block_size(),
        live: pool.live_count(),
        free: pool.free_count(),
        acquired_total: pool.acquired_total(),
        reserved_bytes: pool.reserved_bytes(),
    }
}

fn delta(after: u64, before: u64) -> i64 {
    after as i64 - before as i64
}

/// Registers a named allocation tracker, and makes it the one the plain functions work on.
/// `false` when the name is already registered.
pub fn register_allocation_tracker(name: &str, budget: Option<u64>) -> bool {
    with_state(|state| {
        if state.trackers.contains_key(name) {
            return false;
        }
        let ledger = Ledger {
            budget,
            ..Ledger::default()
        };
        state.trackers.insert(name.to_owned(), ledger);
        state.current = Some(name.to_owned());
        true
    })
}

/// Makes a registered tracker the one the plain functions work on. `false` when it is not
/// registered.
pub fn select_tracker(name: &str) -> bool {
    with_state(|state| {
        if !state.trackers.contains_key(name) {
            return false;
        }
        state.current = Some(name.to_owned());
        true
    })
}

/// Declares an allocation into the current tracker, answering its id. A declaration past the
/// budget is refused and changes nothing.
pub fn track(name: &str, size: u64, owner: &str) -> Result<u64, TrackError> {
    with_state(|state| {
        let handlers: Vec<(String, u64)> = state
            .handlers
            .iter()
            .map(|handler| (handler.name.clone(), handler.threshold))
            .collect();
        let previous = state
            .trackers
            .get(state.current.as_deref().unwrap_or(""))
            .map_or(0, |ledger| ledger.current);
        let (id, now) = {
            let ledger = ensure_current(state);
            if let Some(budget) = ledger.budget {
                if ledger.current.saturating_add(size) > budget {
                    return Err(TrackError::BudgetExceeded {
                        requested: size,
                        current: ledger.current,
                        budget,
                    });
                }
            }
            let id = ledger.next_id;
            ledger.next_id += 1;
            ledger.entries.insert(
                id,
                Tracked {
                    name: name.to_owned(),
                    size,
                    owner: owner.to_owned(),
                },
            );
            ledger.current += size;
            ledger.total += size;
            ledger.peak = ledger.peak.max(ledger.current);
            (id, ledger.current)
        };
        let fired: Vec<PressureEvent> = handlers
            .into_iter()
            .filter(|(_, threshold)| previous < *threshold && now >= *threshold)
            .map(|(handler, threshold)| PressureEvent {
                handler,
                threshold,
                current: now,
            })
            .collect();
        state.events.extend(fired);
        Ok(id)
    })
}

/// Releases an allocation by id, answering whether it was held.
pub fn untrack(id: u64) -> bool {
    with_state(|state| {
        let ledger = ensure_current(state);
        match ledger.entries.remove(&id) {
            Some(entry) => {
                ledger.current = ledger.current.saturating_sub(entry.size);
                true
            }
            None => false,
        }
    })
}

/// Registers a named threshold over the mod's own tracked bytes. `false` when the name is
/// already registered.
pub fn register_pressure_handler(name: &str, threshold: u64) -> bool {
    with_state(|state| {
        if state
            .handlers
            .iter()
            .any(|handler| handler.name == name)
        {
            return false;
        }
        state.handlers.push(PressureHandler {
            name: name.to_owned(),
            threshold,
        });
        true
    })
}

/// The threshold crossings so far, oldest first.
#[must_use]
pub fn get_pressure_events() -> Vec<PressureEvent> {
    with_state(|state| state.events.clone())
}

/// The threshold crossings so far, and forgets them.
#[must_use]
pub fn take_pressure_events() -> Vec<PressureEvent> {
    with_state(|state| std::mem::take(&mut state.events))
}

/// The bytes the current tracker still holds.
#[must_use]
pub fn get_allocated() -> u64 {
    with_state(|state| current_ledger(state).map_or(0, |ledger| ledger.current))
}

/// The high-water mark the current tracker ever reached.
#[must_use]
pub fn get_peak_usage() -> u64 {
    with_state(|state| current_ledger(state).map_or(0, |ledger| ledger.peak))
}

/// Every byte the current tracker ever accepted.
#[must_use]
pub fn get_total_bytes() -> u64 {
    with_state(|state| current_ledger(state).map_or(0, |ledger| ledger.total))
}

/// The allocations the current tracker still holds.
#[must_use]
pub fn get_allocation_count() -> usize {
    with_state(|state| current_ledger(state).map_or(0, |ledger| ledger.entries.len()))
}

/// What the current tracker declared and never released, in declaration order.
#[must_use]
pub fn get_leak_report() -> Vec<Leak> {
    with_state(|state| {
        current_ledger(state)
            .map(|ledger| {
                ledger
                    .entries
                    .iter()
                    .map(|(id, entry)| Leak {
                        id: *id,
                        name: entry.name.clone(),
                        size: entry.size,
                        owner: entry.owner.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

/// What each owner still holds, by owner name.
#[must_use]
pub fn per_owner_breakdown() -> Vec<OwnerBytes> {
    with_state(|state| {
        let Some(ledger) = current_ledger(state) else {
            return Vec::new();
        };
        let mut owners: BTreeMap<String, (u64, usize)> = BTreeMap::new();
        for entry in ledger.entries.values() {
            let slot = owners.entry(entry.owner.clone()).or_default();
            slot.0 += entry.size;
            slot.1 += 1;
        }
        owners
            .into_iter()
            .map(|(owner, (bytes, allocations))| OwnerBytes {
                owner,
                bytes,
                allocations,
            })
            .collect()
    })
}

/// The current tracker as a moment.
#[must_use]
pub fn create_snapshot() -> Snapshot {
    with_state(|state| {
        let Some(ledger) = current_ledger(state) else {
            return Snapshot::default();
        };
        let mut owners: BTreeMap<String, u64> = BTreeMap::new();
        for entry in ledger.entries.values() {
            *owners.entry(entry.owner.clone()).or_default() += entry.size;
        }
        Snapshot {
            tracker: state.current.clone().unwrap_or_default(),
            current: ledger.current,
            peak: ledger.peak,
            total: ledger.total,
            allocations: ledger.entries.len(),
            owners,
        }
    })
}

/// What moved between two snapshots, per owner and in total.
#[must_use]
pub fn compare_snapshots(before: &Snapshot, after: &Snapshot) -> SnapshotDiff {
    let owners: BTreeSet<&String> = before.owners.keys().chain(after.owners.keys()).collect();
    SnapshotDiff {
        current_delta: delta(after.current, before.current),
        peak_delta: delta(after.peak, before.peak),
        total_delta: delta(after.total, before.total),
        allocations_delta: delta(after.allocations as u64, before.allocations as u64),
        owners: owners
            .into_iter()
            .map(|owner| {
                let before = before.owners.get(owner).copied().unwrap_or(0);
                let after = after.owners.get(owner).copied().unwrap_or(0);
                OwnerDelta {
                    owner: owner.clone(),
                    before,
                    after,
                    delta: delta(after, before),
                }
            })
            .collect(),
    }
}

/// Registers a named arena over a block of `capacity` bytes. `false` when the name is taken.
pub fn create_arena(name: &str, capacity: usize) -> bool {
    with_state(|state| {
        if state.arenas.contains_key(name) {
            return false;
        }
        state.arenas.insert(name.to_owned(), Arena::new(capacity));
        true
    })
}

/// Allocates in a registered arena.
pub fn arena_alloc(name: &str, size: usize, align: usize) -> Result<usize, AllocError> {
    with_state(|state| {
        let arena = state
            .arenas
            .get_mut(name)
            .ok_or_else(|| AllocError::UnknownBuffer(name.to_owned()))?;
        arena.alloc(size, align)
    })
}

/// Gives a registered arena's whole block back, answering the bytes that were handed out.
pub fn arena_reset(name: &str) -> Result<usize, AllocError> {
    with_state(|state| {
        let arena = state
            .arenas
            .get_mut(name)
            .ok_or_else(|| AllocError::UnknownBuffer(name.to_owned()))?;
        Ok(arena.reset())
    })
}

/// Registers a named slab over a block of `capacity` bytes. `false` when the name is taken.
pub fn create_slab(name: &str, capacity: usize) -> bool {
    with_state(|state| {
        if state.slabs.contains_key(name) {
            return false;
        }
        state.slabs.insert(name.to_owned(), Slab::new(capacity));
        true
    })
}

/// Allocates in a registered slab.
pub fn slab_alloc(name: &str, size: usize, align: usize) -> Result<usize, AllocError> {
    with_state(|state| {
        let slab = state
            .slabs
            .get_mut(name)
            .ok_or_else(|| AllocError::UnknownBuffer(name.to_owned()))?;
        slab.alloc(size, align)
    })
}

/// Returns a block to a registered slab's free list, answering its size.
pub fn slab_free(name: &str, offset: usize) -> Result<usize, AllocError> {
    with_state(|state| {
        let slab = state
            .slabs
            .get_mut(name)
            .ok_or_else(|| AllocError::UnknownBuffer(name.to_owned()))?;
        slab.free(offset)
    })
}

/// The bytes a registered slab holds, where it is registered.
#[must_use]
pub fn slab_used(name: &str) -> Option<usize> {
    with_state(|state| state.slabs.get(name).map(Slab::used))
}

/// The bytes a registered slab has free, where it is registered.
#[must_use]
pub fn slab_free_bytes(name: &str) -> Option<usize> {
    with_state(|state| state.slabs.get(name).map(Slab::free_bytes))
}

/// Registers a named pool of `slots` fixed-size blocks. `false` when the name is taken.
pub fn create_pool(name: &str, block_size: usize, slots: usize) -> bool {
    with_state(|state| {
        if state.pools.contains_key(name) {
            return false;
        }
        state.pools.insert(name.to_owned(), Pool::new(block_size, slots));
        true
    })
}

/// Takes a slot in a registered pool.
pub fn pool_acquire(name: &str) -> Result<usize, PoolError> {
    with_state(|state| {
        let pool = state
            .pools
            .get_mut(name)
            .ok_or_else(|| PoolError::UnknownPool(name.to_owned()))?;
        pool.acquire()
    })
}

/// Returns a slot to a registered pool.
pub fn pool_release(name: &str, slot: usize) -> Result<(), PoolError> {
    with_state(|state| {
        let pool = state
            .pools
            .get_mut(name)
            .ok_or_else(|| PoolError::UnknownPool(name.to_owned()))?;
        pool.release(slot)
    })
}

/// Every registered pool, by name.
#[must_use]
pub fn get_pool_stats() -> Vec<PoolStats> {
    with_state(|state| {
        state
            .pools
            .iter()
            .map(|(name, pool)| pool_stats(name, pool))
            .collect()
    })
}

/// Every heap a mod owns, with the total reserved.
#[must_use]
pub fn get_heap_info() -> HeapInfo {
    with_state(|state| HeapInfo {
        arenas: state
            .arenas
            .iter()
            .map(|(name, arena)| ArenaInfo {
                name: name.clone(),
                capacity: arena.capacity(),
                used: arena.used(),
                remaining: arena.remaining(),
            })
            .collect(),
        slabs: state
            .slabs
            .iter()
            .map(|(name, slab)| SlabInfo {
                name: name.clone(),
                capacity: slab.capacity(),
                used: slab.used(),
                free: slab.free_bytes(),
                fragments: slab.fragments(),
            })
            .collect(),
        pools: state
            .pools
            .iter()
            .map(|(name, pool)| pool_stats(name, pool))
            .collect(),
        reserved_bytes: reserved_bytes(state),
    })
}

/// The bytes reserved across every arena, slab and pool the mod registered.
#[must_use]
pub fn get_reserved() -> u64 {
    with_state(|state| reserved_bytes(state))
}

/// The mod's own ledger plus this process's size, as one answer.
#[must_use]
pub fn get_usage() -> Usage {
    let process = crate::diagnostics::get_memory_usage();
    let (tracked, peak, allocations) = with_state(|state| match current_ledger(state) {
        Some(ledger) => (ledger.current, ledger.peak, ledger.entries.len()),
        None => (0, 0, 0),
    });
    Usage {
        tracked_bytes: tracked,
        peak_bytes: peak,
        allocations,
        reserved_bytes: get_reserved(),
        process_resident_bytes: process.resident_bytes,
        process_virtual_bytes: process.virtual_bytes,
    }
}

/// The memory the operating system reports as available, where it reports it.
#[must_use]
pub fn get_available() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix("MemAvailable:")?;
            let digits: String = rest
                .trim()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<u64>().ok().map(|kilobytes| kilobytes * 1024)
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Everything at once: usage, heaps and what the ledger still holds.
#[must_use]
pub fn get_resource_stats() -> ResourceStats {
    let leaks = get_leak_report();
    ResourceStats {
        usage: get_usage(),
        heap: get_heap_info(),
        leaks: leaks.len(),
        leaked_bytes: leaks.iter().map(|leak| leak.size).sum(),
    }
}

/// Usage, heaps and leaks as one answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResourceStats {
    pub usage: Usage,
    pub heap: HeapInfo,
    pub leaks: usize,
    pub leaked_bytes: u64,
}

fn refused(what: &str) {
    crate::warn!(
        "a mod asked this host about memory: {}",
        why_unknown(what)
    );
}

/// The client's own heap metadata. `None`: only the client can read it.
pub fn get_client_heap_info() -> Option<String> {
    refused("client_heap");
    None
}

/// The client's collector statistics. `None`: only the client can read them.
pub fn get_gc_info() -> Option<String> {
    refused("client_gc");
    None
}

/// Forces a collection in the client. `false`: this host will not run the client's collector.
pub fn request_gc() -> bool {
    refused("force_gc");
    false
}

/// The client's memory usage. `None`: only the client can read it.
pub fn get_client_usage() -> Option<u64> {
    refused("client_usage");
    None
}

/// Why a question about the client's own memory has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_heap" => {
            "the client's heap belongs to the client's allocator; this host can see the bytes \
             a mod declares, not the heap the client keeps to itself"
                .to_owned()
        }
        "client_gc" => {
            "the client's collector is inside the client; this host has no view of its \
             generations, its pauses or its thresholds"
                .to_owned()
        }
        "force_gc" => {
            "forcing a collection would run the client's collector from a mod, which this \
             host will not do"
                .to_owned()
        }
        "client_usage" => {
            "the client's memory usage is read by the client, not by a mod; this host reports \
             the process and the mod's own ledger instead"
                .to_owned()
        }
        _ => format!("{what} is not something this host can see about memory"),
    }
}

/// What this host cannot reach about the client's memory.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_heap", "client_gc", "force_gc", "client_usage"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

#[cfg(test)]
fn reset() {
    with_state(|state| *state = State::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        reset();
    }

    #[test]
    fn an_arena_bumps_aligned_and_refuses_what_does_not_fit() {
        let mut arena = Arena::new(100);
        assert_eq!(arena.alloc(16, 8).unwrap(), 0);
        assert_eq!(arena.alloc(1, 1).unwrap(), 16);
        assert_eq!(arena.alloc(8, 16).unwrap(), 32, "17 rounds up to 32");
        assert_eq!(arena.used(), 40);
        assert_eq!(arena.remaining(), 60);
        assert_eq!(
            arena.alloc(61, 1),
            Err(AllocError::OutOfSpace {
                requested: 61,
                available: 60
            })
        );
        assert_eq!(arena.alloc(8, 4).unwrap(), 40);
        assert_eq!(arena.alloc(1, 3), Err(AllocError::BadAlign(3)));
        assert_eq!(arena.reset(), 48);
        assert_eq!(arena.used(), 0);
        assert_eq!(arena.alloc(8, 8).unwrap(), 0);
    }

    #[test]
    fn a_slab_reuses_a_freed_block_at_the_same_offset() {
        let mut slab = Slab::new(64);
        assert_eq!(slab.alloc(16, 8).unwrap(), 0);
        assert_eq!(slab.alloc(16, 8).unwrap(), 16);
        assert_eq!(slab.used(), 32);
        assert_eq!(slab.free_bytes(), 32);
        assert_eq!(slab.fragments(), 1);
        assert_eq!(slab.free(0).unwrap(), 16);
        assert_eq!(slab.used(), 16);
        assert_eq!(slab.free_bytes(), 48);
        assert_eq!(
            slab.alloc(16, 8).unwrap(),
            0,
            "the freed block comes back at its own offset"
        );
        assert_eq!(slab.used(), 32);
        assert_eq!(slab.free(4), Err(AllocError::UnknownOffset(4)));
        assert_eq!(
            slab.alloc(64, 1),
            Err(AllocError::OutOfSpace {
                requested: 64,
                available: 32
            })
        );
    }

    #[test]
    fn a_slab_coalesces_neighbours_and_still_fragments() {
        let mut slab = Slab::new(48);
        assert_eq!(slab.alloc(16, 8).unwrap(), 0);
        assert_eq!(slab.alloc(16, 8).unwrap(), 16);
        assert_eq!(slab.alloc(16, 8).unwrap(), 32);
        assert_eq!(slab.free(16).unwrap(), 16);
        assert_eq!(slab.free(32).unwrap(), 16);
        assert_eq!(slab.fragments(), 1, "two freed neighbours become one run");
        assert_eq!(slab.free_bytes(), 32);
        assert_eq!(
            slab.alloc(32, 8).unwrap(),
            16,
            "the coalesced run holds a larger block"
        );
        assert_eq!(slab.used(), 48);

        let mut split = Slab::new(64);
        assert_eq!(split.alloc(16, 8).unwrap(), 0);
        assert_eq!(split.alloc(16, 8).unwrap(), 16);
        assert_eq!(split.alloc(16, 8).unwrap(), 32);
        assert_eq!(split.alloc(16, 8).unwrap(), 48);
        assert_eq!(split.free(0).unwrap(), 16);
        assert_eq!(split.free(32).unwrap(), 16);
        assert_eq!(split.fragments(), 2);
        assert_eq!(split.free_bytes(), 32);
        assert_eq!(
            split.alloc(24, 8),
            Err(AllocError::OutOfSpace {
                requested: 24,
                available: 32
            }),
            "32 bytes free is not one 24-byte run"
        );
        assert_eq!(split.alloc(16, 8).unwrap(), 0, "the lowest free run is used");
    }

    #[test]
    fn the_ledger_reckons_current_peak_total_and_per_owner() {
        let _serial = serial();
        fresh();
        assert!(register_allocation_tracker("mod", None));
        assert!(
            !register_allocation_tracker("mod", None),
            "the same name is not registered twice"
        );
        let before = create_snapshot();

        assert_eq!(track("mesh", 100, "render").unwrap(), 0);
        assert_eq!(track("texture", 40, "render").unwrap(), 1);
        assert!(register_pressure_handler("watch", 160));
        assert!(!register_pressure_handler("watch", 160));
        assert_eq!(track("chunk", 60, "world").unwrap(), 2);
        assert_eq!(get_allocated(), 200);
        assert_eq!(get_peak_usage(), 200);
        assert_eq!(get_total_bytes(), 200);
        assert_eq!(get_allocation_count(), 3);

        assert!(untrack(1));
        assert_eq!(get_allocated(), 160);
        assert_eq!(get_peak_usage(), 200, "the peak remembers the high-water mark");
        assert_eq!(get_total_bytes(), 200, "total bytes only ever grow");
        assert!(!untrack(1), "an allocation is released once");

        let owners = per_owner_breakdown();
        assert_eq!(owners.len(), 2);
        assert_eq!(
            owners[0],
            OwnerBytes {
                owner: "render".to_owned(),
                bytes: 100,
                allocations: 1
            }
        );
        assert_eq!(
            owners[1],
            OwnerBytes {
                owner: "world".to_owned(),
                bytes: 60,
                allocations: 1
            }
        );

        let after = create_snapshot();
        assert_eq!(after.current, 160);
        let diff = compare_snapshots(&before, &after);
        assert_eq!(diff.current_delta, 160);
        assert_eq!(diff.total_delta, 200);
        assert_eq!(diff.allocations_delta, 2);
        assert_eq!(diff.owners.len(), 2);

        let fired = take_pressure_events();
        assert_eq!(fired.len(), 1);
        assert_eq!(
            fired[0],
            PressureEvent {
                handler: "watch".to_owned(),
                threshold: 160,
                current: 200
            }
        );
        assert!(get_pressure_events().is_empty(), "taking them clears the list");
    }

    #[test]
    fn a_budget_refuses_a_declaration_past_it() {
        let _serial = serial();
        fresh();
        assert!(register_allocation_tracker("mod", Some(100)));
        assert_eq!(track("a", 60, "mod").unwrap(), 0);
        assert_eq!(get_allocated(), 60);
        assert_eq!(
            track("b", 60, "mod"),
            Err(TrackError::BudgetExceeded {
                requested: 60,
                current: 60,
                budget: 100
            })
        );
        assert_eq!(get_allocated(), 60, "a refused declaration changes nothing");
        assert_eq!(get_total_bytes(), 60);
        assert_eq!(track("c", 40, "mod").unwrap(), 1);
        assert_eq!(get_allocated(), 100);
        assert_eq!(
            track("d", 1, "mod"),
            Err(TrackError::BudgetExceeded {
                requested: 1,
                current: 100,
                budget: 100
            })
        );
        assert_eq!(get_peak_usage(), 100);
    }

    #[test]
    fn the_leak_report_names_what_was_never_freed_in_order() {
        let _serial = serial();
        fresh();
        register_allocation_tracker("mod", None);
        let first = track("first", 10, "mod").unwrap();
        let second = track("second", 20, "mod").unwrap();
        let third = track("third", 30, "mod").unwrap();
        assert!(untrack(second));
        let leaks = get_leak_report();
        assert_eq!(leaks.len(), 2);
        assert_eq!(
            leaks[0],
            Leak {
                id: first,
                name: "first".to_owned(),
                size: 10,
                owner: "mod".to_owned()
            }
        );
        assert_eq!(
            leaks[1],
            Leak {
                id: third,
                name: "third".to_owned(),
                size: 30,
                owner: "mod".to_owned()
            }
        );
        assert_eq!(get_resource_stats().leaked_bytes, 40);
    }

    #[test]
    fn a_pool_reuses_a_slot_and_the_registry_sizes_the_heaps() {
        let _serial = serial();
        fresh();
        let mut pool = Pool::new(8, 2);
        assert_eq!(pool.acquire().unwrap(), 0);
        assert_eq!(pool.acquire().unwrap(), 1);
        assert_eq!(pool.live_count(), 2);
        assert_eq!(pool.free_count(), 0);
        assert_eq!(pool.acquire(), Err(PoolError::Exhausted { capacity: 2 }));
        assert_eq!(pool.offset_of(1), Some(8));
        assert_eq!(pool.release(0), Ok(()));
        assert_eq!(pool.live_count(), 1);
        assert_eq!(pool.acquire().unwrap(), 0, "the released slot comes back");
        assert_eq!(pool.acquired_total(), 3);
        assert_eq!(pool.release(0), Ok(()));
        assert_eq!(pool.release(0), Err(PoolError::UnknownSlot(0)));
        assert_eq!(pool.offset_of(9), None);

        assert!(create_arena("bump", 256));
        assert!(!create_arena("bump", 256), "a name is used once");
        assert_eq!(arena_alloc("bump", 16, 8).unwrap(), 0);
        assert_eq!(arena_alloc("bump", 8, 16).unwrap(), 16);
        assert_eq!(
            arena_alloc("missing", 1, 1),
            Err(AllocError::UnknownBuffer("missing".to_owned()))
        );
        assert!(create_slab("list", 128));
        assert_eq!(slab_alloc("list", 32, 8).unwrap(), 0);
        assert_eq!(slab_free("list", 0).unwrap(), 32);
        assert_eq!(slab_alloc("list", 16, 8).unwrap(), 0);
        assert_eq!(slab_used("list"), Some(16));
        assert_eq!(slab_free_bytes("list"), Some(112));
        assert!(create_pool("nodes", 32, 4));
        assert_eq!(pool_acquire("nodes").unwrap(), 0);
        assert_eq!(pool_release("nodes", 0), Ok(()));
        assert_eq!(
            pool_acquire("missing"),
            Err(PoolError::UnknownPool("missing".to_owned()))
        );

        assert_eq!(get_reserved(), 256 + 128 + 32 * 4);
        let heap = get_heap_info();
        assert_eq!(heap.arenas.len(), 1);
        assert_eq!(heap.arenas[0].capacity, 256);
        assert_eq!(heap.slabs[0].used, 16);
        assert_eq!(heap.slabs[0].fragments, 1);
        assert_eq!(heap.pools[0].capacity, 4);
        assert_eq!(heap.reserved_bytes, 512);
        let stats = get_pool_stats();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].live, 0);
        assert_eq!(stats[0].block_size, 32);
        let resources = get_resource_stats();
        assert_eq!(resources.usage.reserved_bytes, 512);
        assert_eq!(resources.heap.arenas.len(), 1);
    }

    #[test]
    fn the_clients_heap_gc_and_memory_usage_are_refused() {
        let _serial = serial();
        assert!(get_client_heap_info().is_none());
        assert!(get_gc_info().is_none());
        assert!(!request_gc());
        assert!(get_client_usage().is_none());
        let reasons = refusals();
        assert_eq!(reasons.len(), 4);
        for (what, why) in &reasons {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(
                why,
                &format!("{what} is not something this host can see about memory")
            );
        }
        assert!(why_unknown("client_gc").contains("collector"));
        assert!(why_unknown("force_gc").contains("collection"));
        assert!(why_unknown("something_else").contains("something_else"));
    }
}
