//! A test harness a mod ships with itself: declare tests, run them, read the report.
//!
//! A test is a name, a tag and a body. [`create_test`] declares one, [`register_test`] adds
//! it to the mod's own registry, and [`run_all`] (or [`run`] for one, or a tag for a subset)
//! runs it. Each body answers through the recorder behind [`assert_that`] and
//! [`assert_eq_values`], so a test can fail without panicking and its check count is real.
//! A body that *does* panic is caught by the runner, recorded as a failure with the panic
//! message, and the tests after it still run.
//!
//! Tests run in a declared order — [`create_test`]'s default is `0`, [`TestCase::order`]
//! changes it — and ties are broken by name so a suite is reproducible. A [fixture](register_fixture)
//! and a test's own setup and teardown run around every body, teardown included when the
//! body fails. [`seeded`] hands a body a [`crate::math::Rng`], so the same seed is the same
//! sequence twice.
//!
//! ```
//! use cinnaloader::testing;
//!
//! testing::clear();
//! assert!(testing::register_test(testing::create_test("adds", "unit", || {
//!     testing::assert_eq_values(2 + 2, 4);
//! })));
//! assert!(testing::register_test(testing::create_test("fails", "unit", || {
//!     testing::assert_eq_values(1, 2);
//! })));
//! let report = testing::run_all();
//! assert_eq!(report.total(), 2);
//! assert_eq!(report.passed(), 1);
//! assert_eq!(report.failed(), 1);
//! assert!(report.to_json().contains("\"failed\":1"));
//! ```
//!
//! What is refused is the client's *own* test machinery: its game-test hooks and the client
//! state a test would read (events, packets, entities, blocks, inventory, the rendered
//! frame, a network trace). A mod cannot reach any of it, so [`run_game_test`] and the
//! `assert_*` calls that would need it warn and answer an empty result, and [`why_unknown`]
//! and [`refusals`] say the same without warning. Everything else here is real and local:
//! the registry, the runner, the mocks and the test clock.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::serialization::{self, Map, Value};

const TICK_MILLIS: u64 = 50;

/// How a test ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Every check held.
    Passed,
    /// A check failed, or the body panicked.
    Failed,
    /// The body asked to be skipped.
    Skipped,
}

impl Outcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "pass",
            Self::Failed => "fail",
            Self::Skipped => "skip",
        }
    }
}

/// One test's result: what it was called, how it ended, and why when it did not pass.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    pub name: String,
    pub tag: String,
    pub outcome: Outcome,
    pub message: Option<String>,
    pub checks: usize,
}

impl TestOutcome {
    #[must_use]
    pub fn passed(&self) -> bool {
        self.outcome == Outcome::Passed
    }

    #[must_use]
    pub fn failed(&self) -> bool {
        self.outcome == Outcome::Failed
    }

    #[must_use]
    pub fn skipped(&self) -> bool {
        self.outcome == Outcome::Skipped
    }
}

/// What a run produced: every test's outcome, in run order.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub results: Vec<TestOutcome>,
}

impl Report {
    #[must_use]
    pub fn total(&self) -> usize {
        self.results.len()
    }

    #[must_use]
    pub fn passed(&self) -> usize {
        self.results.iter().filter(|result| result.passed()).count()
    }

    #[must_use]
    pub fn failed(&self) -> usize {
        self.results.iter().filter(|result| result.failed()).count()
    }

    #[must_use]
    pub fn skipped(&self) -> usize {
        self.results.iter().filter(|result| result.skipped()).count()
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<&TestOutcome> {
        self.results.iter().find(|result| result.name == name)
    }

    /// One line with the counts, for a log or a summary footer.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} tests: {} passed, {} failed, {} skipped",
            self.total(),
            self.passed(),
            self.failed(),
            self.skipped()
        )
    }

    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = self.summary();
        text.push('\n');
        for result in &self.results {
            text.push_str(&format!(
                "[{}] {} ({}) checks={}",
                result.outcome.as_str(),
                result.name,
                result.tag,
                result.checks
            ));
            if let Some(message) = &result.message {
                text.push_str(&format!(": {message}"));
            }
            text.push('\n');
        }
        text
    }

    /// The report as a [`serialization::Value`], the shape [`to_json`](Self::to_json) writes.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut map = Map::new();
        map.insert("total", Value::Int(self.total() as i64));
        map.insert("passed", Value::Int(self.passed() as i64));
        map.insert("failed", Value::Int(self.failed() as i64));
        map.insert("skipped", Value::Int(self.skipped() as i64));
        let results = self
            .results
            .iter()
            .map(|result| {
                let mut entry = Map::new();
                entry.insert("name", Value::str(result.name.as_str()));
                entry.insert("tag", Value::str(result.tag.as_str()));
                entry.insert("outcome", Value::str(result.outcome.as_str()));
                entry.insert("checks", Value::Int(result.checks as i64));
                entry.insert(
                    "message",
                    match &result.message {
                        Some(message) => Value::str(message.as_str()),
                        None => Value::Null,
                    },
                );
                Value::Map(entry)
            })
            .collect();
        map.insert("results", Value::List(results));
        Value::Map(map)
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        serialization::to_json(&self.to_value())
            .unwrap_or_else(|error| format!("{{\"error\":\"{error}\"}}"))
    }
}

/// The counts a test body accumulates: how many checks it made and the first that failed.
#[derive(Debug, Default)]
pub struct Recorder {
    checks: usize,
    failure: Option<String>,
    skipped: Option<String>,
}

impl Recorder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn checks(&self) -> usize {
        self.checks
    }

    #[must_use]
    pub fn first_failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    #[must_use]
    pub fn skipped(&self) -> Option<&str> {
        self.skipped.as_deref()
    }

    /// Records one check and answers whether it held. The first failure is the one kept.
    pub fn check(&mut self, condition: bool, message: impl Into<String>) -> bool {
        self.checks += 1;
        if !condition && self.failure.is_none() {
            self.failure = Some(message.into());
        }
        condition
    }

    pub fn skip(&mut self, message: impl Into<String>) {
        self.skipped = Some(message.into());
    }
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

fn record(condition: bool, message: impl Into<String>) -> bool {
    RECORDER.with(|slot| match slot.borrow_mut().as_mut() {
        Some(recorder) => recorder.check(condition, message),
        None => panic!("a testing assertion ran outside a test"),
    })
}

/// Records a named check. Answers the condition, and is the one a body should reach for.
pub fn assert_that(condition: bool, message: impl Into<String>) -> bool {
    record(condition, message)
}

/// The same as [`assert_eq_values`], for a caller that only wants Java's spelling.
pub fn assert<T: PartialEq<U> + std::fmt::Debug, U: std::fmt::Debug>(a: T, b: U) -> bool {
    assert_eq_values(a, b)
}

/// Records that `a` and `b` are equal, keeping `a != b` as the failure message.
pub fn assert_eq_values<T, U>(a: T, b: U) -> bool
where
    T: PartialEq<U> + std::fmt::Debug,
    U: std::fmt::Debug,
{
    let condition = a == b;
    record(condition, format!("{a:?} != {b:?}"))
}

/// Records that `a` and `b` are equal.
pub fn assert_eq<T, U>(a: T, b: U) -> bool
where
    T: PartialEq<U> + std::fmt::Debug,
    U: std::fmt::Debug,
{
    assert_eq_values(a, b)
}

/// Records that `a` and `b` differ.
pub fn assert_ne<T, U>(a: T, b: U) -> bool
where
    T: PartialEq<U> + std::fmt::Debug,
    U: std::fmt::Debug,
{
    let condition = a != b;
    record(condition, format!("{a:?} == {b:?}"))
}

pub fn assert_true(condition: bool) -> bool {
    record(condition, "expected true")
}

pub fn assert_false(condition: bool) -> bool {
    record(!condition, "expected false")
}

pub fn assert_contains(haystack: &str, needle: &str) -> bool {
    record(haystack.contains(needle), format!("{haystack:?} does not contain {needle:?}"))
}

/// Marks the running test skipped, with a reason, and lets the body return.
pub fn skip(message: &str) {
    RECORDER.with(|slot| {
        if let Some(recorder) = slot.borrow_mut().as_mut() {
            recorder.skip(message);
        }
    });
}

/// How many checks the running test has made so far.
#[must_use]
pub fn checks() -> usize {
    RECORDER.with(|slot| slot.borrow().as_ref().map_or(0, Recorder::checks))
}

/// The running test's first failing message, if it has one.
#[must_use]
pub fn first_failure() -> Option<String> {
    RECORDER.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(Recorder::first_failure)
            .map(str::to_owned)
    })
}

type Body = Box<dyn Fn() + Send + Sync>;

/// A declared test: its name and tag, the order it runs in, and the closures around it.
pub struct TestCase {
    name: String,
    tag: String,
    order: i32,
    body: Body,
    setup: Option<Body>,
    teardown: Option<Body>,
}

impl TestCase {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    #[must_use]
    pub fn order_value(&self) -> i32 {
        self.order
    }

    /// The declared order; lower runs first, and a tie is broken by name.
    #[must_use]
    pub fn order(mut self, order: i32) -> Self {
        self.order = order;
        self
    }

    #[must_use]
    pub fn before(mut self, setup: impl Fn() + Send + Sync + 'static) -> Self {
        self.setup = Some(Box::new(setup));
        self
    }

    #[must_use]
    pub fn after(mut self, teardown: impl Fn() + Send + Sync + 'static) -> Self {
        self.teardown = Some(Box::new(teardown));
        self
    }
}

/// Declares a test named `name` under `tag`, with `body` as its work.
pub fn create_test<F>(name: &str, tag: &str, body: F) -> TestCase
where
    F: Fn() + Send + Sync + 'static,
{
    TestCase {
        name: name.to_owned(),
        tag: tag.to_owned(),
        order: 0,
        body: Box::new(body),
        setup: None,
        teardown: None,
    }
}

struct Fixture {
    name: String,
    setup: Body,
    teardown: Body,
}

#[derive(Default)]
struct Registry {
    tests: Vec<TestCase>,
    fixtures: Vec<Fixture>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn with<T>(body: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = registry()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Adds a test to the mod's registry. A name already taken is refused and answers `false`.
pub fn register_test(test: TestCase) -> bool {
    with(|registry| {
        if registry.tests.iter().any(|known| known.name == test.name) {
            crate::warn!(
                "a test named '{}' is already registered; the second was refused",
                test.name
            );
            return false;
        }
        registry.tests.push(test);
        true
    })
}

/// Registers a setup and teardown pair that wraps every test: setup in registration order,
/// teardown in reverse. A name already taken is refused and answers `false`.
pub fn register_fixture<S, T>(name: &str, setup: S, teardown: T) -> bool
where
    S: Fn() + Send + Sync + 'static,
    T: Fn() + Send + Sync + 'static,
{
    with(|registry| {
        if registry.fixtures.iter().any(|known| known.name == name) {
            crate::warn!("a fixture named '{name}' is already registered; the second was refused");
            return false;
        }
        registry.fixtures.push(Fixture {
            name: name.to_owned(),
            setup: Box::new(setup),
            teardown: Box::new(teardown),
        });
        true
    })
}

/// Every registered test's name, in run order.
#[must_use]
pub fn list() -> Vec<String> {
    with(|registry| {
        let mut names: Vec<(i32, String)> = registry
            .tests
            .iter()
            .map(|test| (test.order, test.name.clone()))
            .collect();
        names.sort();
        names.into_iter().map(|(_, name)| name).collect()
    })
}

/// The registered tests carrying `tag`, in run order.
#[must_use]
pub fn filter_by_tag(tag: &str) -> Vec<String> {
    with(|registry| {
        let mut names: Vec<(i32, String)> = registry
            .tests
            .iter()
            .filter(|test| test.tag == tag)
            .map(|test| (test.order, test.name.clone()))
            .collect();
        names.sort();
        names.into_iter().map(|(_, name)| name).collect()
    })
}

/// How many tests are registered.
#[must_use]
pub fn count() -> usize {
    with(|registry| registry.tests.len())
}

/// Clears the registry, the stored report and the test clock.
pub fn clear() {
    with(|registry| {
        registry.tests.clear();
        registry.fixtures.clear();
    });
    if let Ok(mut last) = last().lock() {
        *last = None;
    }
    if let Ok(mut clock) = clock().lock() {
        *clock = 0;
    }
}

/// Runs every registered test.
#[must_use]
pub fn run_all() -> Report {
    run_filtered(|_| true)
}

/// Runs the test named `name`, or none when nothing carries that name.
#[must_use]
pub fn run(name: &str) -> Report {
    run_filtered(|test| test.name == name)
}

/// Runs the tests tagged `mod`.
#[must_use]
pub fn run_mod_tests() -> Report {
    run_filtered(|test| test.tag == "mod")
}

/// Runs the tests tagged `integration`.
#[must_use]
pub fn run_integration_tests() -> Report {
    run_filtered(|test| test.tag == "integration")
}

/// Refuses: a game test is the client's own, and a mod cannot start or observe one.
#[must_use]
pub fn run_game_test() -> Report {
    crate::warn!("{}", why_unknown("client_test_hooks"));
    Report::default()
}

/// The last run's report, or an empty one when nothing has run.
#[must_use]
pub fn generate_test_report() -> Report {
    last()
        .lock()
        .ok()
        .and_then(|last| last.clone())
        .unwrap_or_default()
}

fn last() -> &'static Mutex<Option<Report>> {
    static LAST: OnceLock<Mutex<Option<Report>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

fn run_filtered(keep: impl Fn(&TestCase) -> bool) -> Report {
    let (mut selected, fixtures) = with(|registry| {
        let mut selected = Vec::new();
        let mut kept = Vec::new();
        for test in registry.tests.drain(..) {
            if keep(&test) {
                selected.push(test);
            } else {
                kept.push(test);
            }
        }
        registry.tests = kept;
        let fixtures = std::mem::take(&mut registry.fixtures);
        selected.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
        (selected, fixtures)
    });

    let mut results = Vec::with_capacity(selected.len());
    for test in &selected {
        results.push(run_one(test, &fixtures));
    }
    let report = Report { results };

    with(|registry| {
        registry.tests.append(&mut selected);
        registry
            .tests
            .sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.name.cmp(&b.name)));
        registry.fixtures = fixtures;
    });
    if let Ok(mut last) = last().lock() {
        *last = Some(report.clone());
    }
    report
}

// A panic in the body must not take the run with it: catch it, keep the message, and let the
// teardowns and the next test happen anyway.
fn run_one(test: &TestCase, fixtures: &[Fixture]) -> TestOutcome {
    RECORDER.with(|slot| *slot.borrow_mut() = Some(Recorder::new()));

    let mut setup_error = None;
    for fixture in fixtures {
        if let Some(message) = caught_message(|| (fixture.setup)()) {
            setup_error = Some(format!("fixture '{}' setup: {message}", fixture.name));
            break;
        }
    }
    if setup_error.is_none() {
        if let Some(setup) = &test.setup {
            setup_error = caught_message(setup);
        }
    }

    let body_error = if setup_error.is_none() {
        caught_message(|| (test.body)())
    } else {
        None
    };

    if let Some(teardown) = &test.teardown {
        let _ = caught_message(teardown);
    }
    for fixture in fixtures.iter().rev() {
        let _ = caught_message(|| (fixture.teardown)());
    }

    let recorder = RECORDER
        .with(|slot| slot.borrow_mut().take())
        .unwrap_or_default();
    let (outcome, message) = if let Some(error) = setup_error {
        (Outcome::Failed, Some(error))
    } else if let Some(error) = body_error {
        (Outcome::Failed, Some(error))
    } else if let Some(failure) = recorder.first_failure() {
        (Outcome::Failed, Some(failure.to_owned()))
    } else if let Some(reason) = recorder.skipped() {
        (Outcome::Skipped, Some(reason.to_owned()))
    } else {
        (Outcome::Passed, None)
    };
    TestOutcome {
        name: test.name.clone(),
        tag: test.tag.clone(),
        outcome,
        message,
        checks: recorder.checks(),
    }
}

fn caught_message(body: impl FnOnce()) -> Option<String> {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(()) => None,
        Err(payload) => Some(panic_message(&*payload)),
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "the test panicked".to_owned()
    }
}

/// The first difference between two reports, or `None` when they match. The string names
/// the test and the field, so a golden comparison points at one line.
#[must_use]
pub fn compare_reports(golden: &Report, actual: &Report) -> Option<String> {
    if golden.total() != actual.total() {
        return Some(format!(
            "test count differs: golden {} != actual {}",
            golden.total(),
            actual.total()
        ));
    }
    for (index, (a, b)) in golden.results.iter().zip(&actual.results).enumerate() {
        if a.name != b.name {
            return Some(format!("test {index}: name '{}' != '{}'", a.name, b.name));
        }
        if a.outcome != b.outcome {
            return Some(format!(
                "test '{}': outcome {} != {}",
                a.name,
                a.outcome.as_str(),
                b.outcome.as_str()
            ));
        }
        if a.message != b.message {
            return Some(format!(
                "test '{}': message {:?} != {:?}",
                a.name, a.message, b.message
            ));
        }
        if a.checks != b.checks {
            return Some(format!(
                "test '{}': checks {} != {}",
                a.name, a.checks, b.checks
            ));
        }
    }
    None
}

/// A random source seeded a test's way: the same seed is the same sequence twice.
#[must_use]
pub fn seeded(seed: u64) -> crate::math::Rng {
    crate::math::Rng::new(seed)
}

fn clock() -> &'static Mutex<u64> {
    static CLOCK: OnceLock<Mutex<u64>> = OnceLock::new();
    CLOCK.get_or_init(|| Mutex::new(0))
}

/// The test clock's current tick.
#[must_use]
pub fn current_tick() -> u64 {
    clock().lock().map(|clock| *clock).unwrap_or(0)
}

fn advance_ticks(ticks: u64) -> u64 {
    let mut clock = clock().lock().unwrap_or_else(|error| error.into_inner());
    *clock += ticks;
    *clock
}

/// Advances the test clock one tick and answers the new tick count.
pub fn tick() -> u64 {
    advance_ticks(1)
}

/// Advances the test clock `ticks` ticks and answers the new tick count.
pub fn tick_n(ticks: u32) -> u64 {
    advance_ticks(u64::from(ticks))
}

/// Advances the test clock by a duration, at fifty milliseconds a tick.
pub fn advance_time(duration: Duration) -> u64 {
    advance_ticks(duration.as_millis() as u64 / TICK_MILLIS)
}

/// Ticks until `condition` holds, or `timeout_ticks` ticks have passed.
pub fn wait_for(condition: impl Fn() -> bool, timeout_ticks: u32) -> bool {
    if condition() {
        return true;
    }
    for _ in 0..timeout_ticks {
        tick();
        if condition() {
            return true;
        }
    }
    false
}

/// A world that lives in memory, for a test that needs one without the client's.
#[derive(Debug, Clone, Default)]
pub struct MockWorld {
    blocks: HashMap<(i32, i32, i32), String>,
    entities: Vec<MockEntity>,
    players: Vec<MockPlayer>,
    ticks: u64,
    weather: String,
}

impl MockWorld {
    #[must_use]
    pub fn new() -> Self {
        Self {
            weather: "clear".to_owned(),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn block_at(&self, at: (i32, i32, i32)) -> Option<&str> {
        self.blocks.get(&at).map(String::as_str)
    }

    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    #[must_use]
    pub fn entities(&self) -> &[MockEntity] {
        &self.entities
    }

    #[must_use]
    pub fn players(&self) -> &[MockPlayer] {
        &self.players
    }

    #[must_use]
    pub fn time(&self) -> u64 {
        self.ticks
    }

    #[must_use]
    pub fn weather(&self) -> &str {
        &self.weather
    }
}

/// A player a test put in a [`MockWorld`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockPlayer {
    pub name: String,
    pub at: (i32, i32, i32),
}

/// An entity a test put in a [`MockWorld`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockEntity {
    pub kind: String,
    pub at: (i32, i32, i32),
}

/// Items a test gave to a mock player.
#[derive(Debug, Clone, Default)]
pub struct MockInventory {
    items: HashMap<String, u32>,
}

impl MockInventory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn count_of(&self, item: &str) -> u32 {
        self.items.get(item).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn total(&self) -> u32 {
        self.items.values().sum()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// A connection that keeps what was sent to it, for a test that needs one.
#[derive(Debug, Clone, Default)]
pub struct MockConnection {
    sent: Vec<String>,
    open: bool,
}

impl MockConnection {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Sends a message when it is open, and answers whether it went.
    pub fn send(&mut self, message: &str) -> bool {
        if !self.open {
            return false;
        }
        self.sent.push(message.to_owned());
        true
    }

    #[must_use]
    pub fn sent(&self) -> Vec<String> {
        self.sent.clone()
    }
}

/// An empty mock world.
#[must_use]
pub fn create_mock_world() -> MockWorld {
    MockWorld::new()
}

/// A mock player named `name` at the origin.
#[must_use]
pub fn create_mock_player(name: &str) -> MockPlayer {
    MockPlayer {
        name: name.to_owned(),
        at: (0, 0, 0),
    }
}

/// A mock entity of kind `kind` at the origin.
#[must_use]
pub fn create_mock_entity(kind: &str) -> MockEntity {
    MockEntity {
        kind: kind.to_owned(),
        at: (0, 0, 0),
    }
}

/// An empty mock inventory.
#[must_use]
pub fn create_mock_inventory() -> MockInventory {
    MockInventory::new()
}

/// A closed mock connection.
#[must_use]
pub fn create_mock_connection() -> MockConnection {
    MockConnection::new()
}

/// Puts `player` in `world` and answers its index.
pub fn spawn_test_player(world: &mut MockWorld, player: MockPlayer) -> usize {
    world.players.push(player);
    world.players.len() - 1
}

/// Sets a block in `world`, answering `false` when the cell already held one.
pub fn place_test_block(world: &mut MockWorld, at: (i32, i32, i32), block: &str) -> bool {
    if world.blocks.contains_key(&at) {
        return false;
    }
    world.blocks.insert(at, block.to_owned());
    true
}

/// Adds `count` of `item` to `inventory` and answers the new total for it.
pub fn give_test_item(inventory: &mut MockInventory, item: &str, count: u32) -> u32 {
    let held = inventory.items.entry(item.to_owned()).or_insert(0);
    *held += count;
    *held
}

/// Sets `world`'s time of day, wrapped to a day.
pub fn set_test_time(world: &mut MockWorld, ticks: u64) {
    world.ticks = ticks % 24_000;
}

/// Sets `world`'s weather, refusing a name the client does not use.
pub fn set_test_weather(world: &mut MockWorld, weather: &str) -> bool {
    if !matches!(weather, "clear" | "rain" | "thunder") {
        return false;
    }
    world.weather = weather.to_owned();
    true
}

/// Refuses: the client's events are the client's, and a mod cannot read them from here.
#[must_use]
pub fn assert_event_fired(_event: &str) -> bool {
    refuse_state("event")
}

/// Refuses: the client's packets are the client's, and a mod cannot read them from here.
#[must_use]
pub fn assert_packet_sent(_packet: &str) -> bool {
    refuse_state("packet")
}

/// Refuses: the client's entities are the client's, and a mod cannot read them from here.
#[must_use]
pub fn assert_entity_spawned(_kind: &str) -> bool {
    refuse_state("entity")
}

/// Refuses: the client's blocks are the client's, and a mod cannot read them from here.
#[must_use]
pub fn assert_block_state(_at: (i32, i32, i32), _block: &str) -> bool {
    refuse_state("block")
}

/// Refuses: the client's inventory is the client's, and a mod cannot read it from here.
#[must_use]
pub fn assert_inventory(_item: &str, _count: u32) -> bool {
    refuse_state("inventory")
}

fn refuse_state(what: &str) -> bool {
    crate::warn!(
        "a test asked for the {what} the client saw: {}",
        why_unknown("client_test_state")
    );
    false
}

/// Refuses: the rendered frame is the client's, and a mod cannot capture it from here.
#[must_use]
pub fn capture_screenshot() -> Option<Vec<u8>> {
    crate::warn!("{}", why_unknown("screenshot"));
    None
}

/// Refuses: comparing frames needs the client's renderer, which a mod does not have.
#[must_use]
pub fn compare_screenshot(_golden: &[u8], _actual: &[u8]) -> Option<String> {
    crate::warn!("{}", why_unknown("screenshot"));
    Some(why_unknown("screenshot"))
}

/// Refuses: a trace records the client's own traffic, which a mod cannot capture.
#[must_use]
pub fn record_trace() -> Option<Vec<u8>> {
    crate::warn!("{}", why_unknown("trace"));
    None
}

/// Refuses: replaying a trace means driving the client, which a mod cannot do.
pub fn replay_trace(_trace: &[u8]) -> bool {
    crate::warn!("{}", why_unknown("trace"));
    false
}

/// Why the client's own test machinery has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "client_test_hooks" => {
            "a game test is the client's own: a mod cannot start, steer or observe the \
             client's test hooks, and this host has nothing to hand those calls to"
                .to_owned()
        }
        "client_test_state" => {
            "the client's test state belongs to the client's own runner: a mod cannot read \
             its events, packets, entities, blocks or inventory from here"
                .to_owned()
        }
        "screenshot" => {
            "a screenshot is the client's own rendered frame, and this host draws none for a \
             mod's test"
                .to_owned()
        }
        "trace" => {
            "a trace records the client's own network and tick traffic, which no mod-side \
             harness can capture or replay"
                .to_owned()
        }
        _ => format!("{what} is not something this host's test harness can see"),
    }
}

/// What this host's test harness cannot reach, as `(what, sentence)`.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["client_test_hooks", "client_test_state", "screenshot", "trace"]
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

    #[test]
    fn a_run_counts_the_passing_failing_and_skipped_tests() {
        let _serial = serial();
        clear();
        register_test(create_test("passes", "unit", || {
            assert_true(true);
        }));
        register_test(create_test("fails", "unit", || {
            assert_eq_values(1, 2);
        }));
        register_test(create_test("skips", "unit", || {
            skip("needs the client");
        }));
        let report = run_all();
        assert_eq!(report.total(), 3);
        assert_eq!(report.passed(), 1);
        assert_eq!(report.failed(), 1);
        assert_eq!(report.skipped(), 1);
        assert!(report.get("passes").unwrap().passed());
        assert_eq!(report.get("fails").unwrap().message.as_deref(), Some("1 != 2"));
        assert_eq!(
            report.get("skips").unwrap().message.as_deref(),
            Some("needs the client")
        );
        assert_eq!(generate_test_report().total(), 3);
    }

    #[test]
    fn a_duplicate_name_is_refused() {
        let _serial = serial();
        clear();
        assert!(register_test(create_test("once", "unit", || {})));
        assert!(!register_test(create_test("once", "unit", || {})), "per name, once");
        assert_eq!(count(), 1);
    }

    #[test]
    fn a_panicking_test_is_isolated_and_the_rest_still_run() {
        let _serial = serial();
        clear();
        let ran = Arc::new(AtomicUsize::new(0));
        register_test(create_test("before", "unit", || {
            assert_true(true);
        }));
        register_test(create_test("boom", "unit", || panic!("boom went off")));
        let counter = Arc::clone(&ran);
        register_test(create_test("after", "unit", move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        let report = run_all();
        assert_eq!(report.total(), 3, "the run did not stop at the panic");
        assert_eq!(report.passed(), 2, "the neighbours passed");
        assert_eq!(report.failed(), 1);
        assert_eq!(
            report.get("boom").unwrap().message.as_deref(),
            Some("boom went off"),
            "the panic message was kept"
        );
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the test after the panic ran");
    }

    #[test]
    fn listing_filters_by_tag_and_orders_by_declared_then_name() {
        let _serial = serial();
        clear();
        register_test(create_test("zulu", "slow", || {}).order(1));
        register_test(create_test("alpha", "slow", || {}).order(1));
        register_test(create_test("first", "fast", || {}).order(0));
        assert_eq!(list(), vec!["first", "alpha", "zulu"]);
        assert_eq!(filter_by_tag("slow"), vec!["alpha", "zulu"]);
        assert_eq!(filter_by_tag("fast"), vec!["first"]);
        assert!(filter_by_tag("none").is_empty());
        assert_eq!(count(), 3);
    }

    #[test]
    fn setup_and_teardown_run_around_each_test_and_after_a_failure() {
        let _serial = serial();
        clear();
        let trace = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let fixture_setup = Arc::clone(&trace);
        let fixture_teardown = Arc::clone(&trace);
        assert!(register_fixture(
            "world",
            move || fixture_setup.lock().unwrap().push("fixture-setup".to_owned()),
            move || fixture_teardown.lock().unwrap().push("fixture-teardown".to_owned()),
        ));
        let pass_body = Arc::clone(&trace);
        let pass_setup = Arc::clone(&trace);
        let pass_teardown = Arc::clone(&trace);
        register_test(
            create_test("passing", "unit", move || {
                pass_body.lock().unwrap().push("body:passing".to_owned());
            })
            .order(1)
            .before(move || pass_setup.lock().unwrap().push("setup:passing".to_owned()))
            .after(move || pass_teardown.lock().unwrap().push("teardown:passing".to_owned())),
        );
        let fail_body = Arc::clone(&trace);
        let fail_teardown = Arc::clone(&trace);
        register_test(
            create_test("failing", "unit", move || {
                fail_body.lock().unwrap().push("body:failing".to_owned());
                panic!("failing body");
            })
            .order(2)
            .after(move || fail_teardown.lock().unwrap().push("teardown:failing".to_owned())),
        );
        let report = run_all();
        assert_eq!(report.failed(), 1);
        assert_eq!(
            *trace.lock().unwrap(),
            vec![
                "fixture-setup",
                "setup:passing",
                "body:passing",
                "teardown:passing",
                "fixture-teardown",
                "fixture-setup",
                "body:failing",
                "teardown:failing",
                "fixture-teardown",
            ],
            "setup wraps the body, teardown still runs after a panic, and the fixture brackets both"
        );
    }

    #[test]
    fn assertions_count_checks_and_answer_the_first_failure() {
        let _serial = serial();
        clear();
        register_test(create_test("checks", "unit", || {
            assert_that(true, "first holds");
            assert_eq_values(1, 2);
            assert_that(false, "third fails too");
            assert_eq!(checks(), 3);
            assert_eq!(first_failure().as_deref(), Some("1 != 2"), "the first failure is kept");
        }));
        let report = run_all();
        let result = report.get("checks").unwrap();
        assert_eq!(result.checks, 3);
        assert!(result.failed());
        assert_eq!(result.message.as_deref(), Some("1 != 2"));
    }

    #[test]
    fn a_seeded_source_repeats_for_the_same_seed() {
        let mut first = seeded(42);
        let mut second = seeded(42);
        let one: Vec<u64> = (0..4).map(|_| first.next_u64()).collect();
        let two: Vec<u64> = (0..4).map(|_| second.next_u64()).collect();
        assert_eq!(one, two, "the same seed is the same sequence twice");
        let mut other = seeded(43);
        assert_ne!(one, (0..4).map(|_| other.next_u64()).collect::<Vec<_>>());
    }

    #[test]
    fn a_report_renders_text_json_and_diffs_by_name() {
        let _serial = serial();
        clear();
        register_test(create_test("alpha", "unit", || {
            assert_true(true);
        }));
        let report = run("alpha");
        assert_eq!(report.total(), 1);
        assert!(report.to_text().contains("[pass] alpha (unit) checks=1"));
        assert_eq!(
            report.summary(),
            "1 tests: 1 passed, 0 failed, 0 skipped"
        );
        let json = report.to_json();
        assert!(json.contains("\"passed\":1"), "{json}");
        assert!(json.contains("\"outcome\":\"pass\""));

        let mut changed = report.clone();
        changed.results[0].outcome = Outcome::Failed;
        changed.results[0].message = Some("tampered".to_owned());
        let diff = compare_reports(&report, &changed);
        assert_eq!(
            diff.as_deref(),
            Some("test 'alpha': outcome pass != fail"),
            "the diff names the one changed test"
        );
        assert!(compare_reports(&report, &report).is_none());
    }

    #[test]
    fn the_mocks_and_clock_are_real_and_the_client_test_calls_are_refused() {
        let _serial = serial();
        clear();
        let mut world = create_mock_world();
        assert!(place_test_block(&mut world, (1, 2, 3), "stone"));
        assert!(!place_test_block(&mut world, (1, 2, 3), "dirt"), "one block per cell");
        assert_eq!(world.block_at((1, 2, 3)), Some("stone"));
        assert_eq!(spawn_test_player(&mut world, create_mock_player("alex")), 0);
        assert_eq!(world.players().len(), 1);
        let mut inventory = create_mock_inventory();
        assert_eq!(give_test_item(&mut inventory, "torch", 4), 4);
        assert_eq!(give_test_item(&mut inventory, "torch", 3), 7);
        set_test_time(&mut world, 25_000);
        assert_eq!(world.time(), 1_000);
        assert!(set_test_weather(&mut world, "rain"));
        assert!(!set_test_weather(&mut world, "snow"), "the client has three states");
        let mut connection = create_mock_connection();
        assert!(!connection.send("early"), "closed");
        connection.open();
        assert!(connection.send("hello"));
        assert_eq!(connection.sent(), vec!["hello".to_owned()]);

        assert_eq!(advance_time(Duration::from_millis(150)), 3);
        let target = 4;
        assert!(wait_for(|| current_tick() >= target, 10));
        assert!(!wait_for(|| false, 2));

        assert!(run_game_test().results.is_empty());
        assert!(!assert_event_fired("jump"));
        assert!(!assert_packet_sent("chat"));
        assert!(!assert_entity_spawned("pig"));
        assert!(!assert_block_state((0, 0, 0), "stone"));
        assert!(!assert_inventory("torch", 1));
        assert!(capture_screenshot().is_none());
        assert!(compare_screenshot(&[1], &[2]).is_some());
        assert!(record_trace().is_none());
        assert!(!replay_trace(&[]));
        assert_eq!(refusals().len(), 4);
        assert_eq!(
            why_unknown("screenshot"),
            refusals()[2].1,
            "the list and the lookup agree"
        );
        assert!(why_unknown("nothing").contains("nothing"));
    }
}
