//! Section 70: the testing harness a mod ships with itself, and a run that proves it.

use cinnaloader::testing;
use std::sync::{Arc, Mutex};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    build_suite();
    let report = testing::run_all();
    lines.push(format!(
        "run all: {} passed, {} failed, {} skipped of {} | verdict {}",
        report.passed(),
        report.failed(),
        report.skipped(),
        report.total(),
        if report.failed() == 0 { "clean" } else { "as expected" },
    ));

    let outcomes = report
        .results
        .iter()
        .map(|result| match &result.message {
            Some(message) => {
                format!("{}={}({}, {:?})", result.name, result.outcome.as_str(), result.checks, message)
            }
            None => format!("{}={}({})", result.name, result.outcome.as_str(), result.checks),
        })
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!("outcomes: {outcomes}"));

    let gamma = report.get("gamma").expect("gamma ran");
    lines.push(format!(
        "panic isolation: 'gamma' failed with {:?} while 'alpha' passed and 'delta' skipped in the same run, so the panic did not stop the suite",
        gamma.message.as_deref().unwrap_or(""),
    ));

    let alpha_json = testing::run("alpha").to_json();
    lines.push(format!("json (one test): {alpha_json}"));

    let mut changed = report.clone();
    if let Some(first) = changed.results.first_mut() {
        first.outcome = testing::Outcome::Failed;
        first.message = Some("tampered".to_owned());
    }
    lines.push(format!(
        "snapshot diff names one changed test: {:?}",
        testing::compare_reports(&report, &changed)
    ));

    lines.push(format!("sequence: {}", setup_teardown_sequence()));

    let mut first = testing::seeded(42);
    let mut second = testing::seeded(42);
    let one: Vec<u64> = (0..3).map(|_| first.next_u64()).collect();
    let two: Vec<u64> = (0..3).map(|_| second.next_u64()).collect();
    lines.push(format!(
        "seeded 42 twice: one=[{:x},{:x},{:x}] two=[{:x},{:x},{:x}] equal={} first_unchanged={}",
        one[0],
        one[1],
        one[2],
        two[0],
        two[1],
        two[2],
        one == two,
        one[0] == two[0],
    ));

    let mut world = testing::create_mock_world();
    testing::place_test_block(&mut world, (0, 64, 0), "stone");
    testing::spawn_test_player(&mut world, testing::create_mock_player("alex"));
    testing::set_test_time(&mut world, 25_000);
    testing::set_test_weather(&mut world, "rain");
    let mut inventory = testing::create_mock_inventory();
    testing::give_test_item(&mut inventory, "torch", 7);
    let mut connection = testing::create_mock_connection();
    connection.open();
    connection.send("hello");
    lines.push(format!(
        "mocks: blocks={} players={} time={} weather={} | torches={} | sent={:?}",
        world.block_count(),
        world.players().len(),
        world.time(),
        world.weather(),
        inventory.count_of("torch"),
        connection.sent(),
    ));

    let refused = testing::refusals()
        .into_iter()
        .map(|(what, _)| what)
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!(
        "refusals: [{refused}] | run_game_test={} | capture_screenshot={:?} | replay_trace={} | assert_block_state={} | assert_event_fired={}",
        testing::run_game_test().total(),
        testing::capture_screenshot(),
        testing::replay_trace(&[]),
        testing::assert_block_state((0, 0, 0), "stone"),
        testing::assert_event_fired("jump"),
    ));

    lines
}

fn build_suite() {
    testing::clear();
    testing::register_test(testing::create_test("alpha", "unit", || {
        testing::assert_true(true);
        testing::assert_eq_values(2 + 2, 4);
        testing::assert_contains("cinnaloader", "load");
    }));
    testing::register_test(testing::create_test("beta", "unit", || {
        testing::assert_true(true);
        testing::assert_eq_values(1, 2);
        testing::assert_true(true);
    }));
    testing::register_test(testing::create_test("gamma", "unit", || {
        panic!("gamma exploded");
    }));
    testing::register_test(testing::create_test("delta", "unit", || {
        testing::skip("needs the client");
    }));
}

fn setup_teardown_sequence() -> String {
    testing::clear();
    let trace = Arc::new(Mutex::new(Vec::<String>::new()));
    let fixture_setup = Arc::clone(&trace);
    let fixture_teardown = Arc::clone(&trace);
    testing::register_fixture(
        "world",
        move || fixture_setup.lock().unwrap().push("fixture-setup".to_owned()),
        move || {
            fixture_teardown
                .lock()
                .unwrap()
                .push("fixture-teardown".to_owned())
        },
    );

    let body_ok = Arc::clone(&trace);
    let setup_ok = Arc::clone(&trace);
    let teardown_ok = Arc::clone(&trace);
    testing::register_test(
        testing::create_test("ok", "unit", move || {
            body_ok.lock().unwrap().push("body:ok".to_owned());
        })
        .order(1)
        .before(move || setup_ok.lock().unwrap().push("setup:ok".to_owned()))
        .after(move || teardown_ok.lock().unwrap().push("teardown:ok".to_owned())),
    );

    let body_fail = Arc::clone(&trace);
    let setup_fail = Arc::clone(&trace);
    let teardown_fail = Arc::clone(&trace);
    testing::register_test(
        testing::create_test("fail", "unit", move || {
            body_fail.lock().unwrap().push("body:fail".to_owned());
            panic!("intended");
        })
        .order(2)
        .before(move || setup_fail.lock().unwrap().push("setup:fail".to_owned()))
        .after(move || teardown_fail.lock().unwrap().push("teardown:fail".to_owned())),
    );

    let _ = testing::run_all();
    let sequence = trace.lock().unwrap().join(" -> ");
    sequence
}
