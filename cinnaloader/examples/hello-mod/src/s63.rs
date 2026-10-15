//! Section 63: time arithmetic, a mod-owned clock, schedulers and timers, with proof.

use cinnaloader::time;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let ticks = 37u64;
    lines.push(format!(
        "time: {} ticks = {} ms = {} s | round trips ms {} s {} duration {} | half tick: 25 ms -> {} tick, 24 ms -> {} tick",
        ticks,
        time::ticks_to_millis(ticks),
        time::ticks_to_seconds(ticks),
        time::millis_to_ticks(time::ticks_to_millis(ticks)),
        time::seconds_to_ticks(time::ticks_to_seconds(ticks)),
        time::from_duration(time::to_duration(ticks)),
        time::millis_to_ticks(25),
        time::millis_to_ticks(24),
    ));

    let mut clock = time::Clock::new();
    clock.advance(13_000);
    clock.advance(11_500);
    lines.push(format!(
        "clock: +13000 then +11500 -> {} ticks, {} ms, day {}, time_of_day {} (crossed midnight at 24000)",
        clock.ticks(),
        clock.millis(),
        clock.day(),
        clock.time_of_day(),
    ));

    let noon = time::Clock::from_ticks(6_000);
    let dusk = time::Clock::from_ticks(18_500);
    lines.push(format!(
        "day: 6000 -> {} {} day={} | 18500 -> {} {} minute={} day={} fraction={:.4} | phase starts {} {} {} {}",
        time::format_time_of_day(noon.time_of_day()),
        noon.phase().as_str(),
        noon.is_day(),
        time::format_time_of_day(dusk.time_of_day()),
        dusk.phase().as_str(),
        dusk.minute(),
        dusk.is_day(),
        dusk.fraction(),
        time::Phase::Day.start_tick(),
        time::Phase::Sunset.start_tick(),
        time::Phase::Night.start_tick(),
        time::Phase::Sunrise.start_tick(),
    ));

    let mut scheduler = time::Scheduler::new();
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
    let fired_150 = scheduler.advance(150);
    let at_150 = (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst));
    let fired_200 = scheduler.advance(200);
    let at_350 = (first.load(Ordering::SeqCst), second.load(Ordering::SeqCst));
    lines.push(format!(
        "scheduler: after 100 and 300 | advance 150 fired {} {:?} | advance 200 fired {} {:?} | pending {}",
        fired_150,
        at_150,
        fired_200,
        at_350,
        scheduler.pending(),
    ));

    let mut beats = time::Scheduler::new();
    let count = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&count);
    let id = beats.every(50, move || {
        c.fetch_add(1, Ordering::SeqCst);
    });
    let fires = beats.advance(150);
    lines.push(format!(
        "repeat: every 50 ticks | advance 150 fired {} (due at 50,100,150) | next due {} = 150 + 50 no drift | cancel {} pending {}",
        fires,
        beats.due_at(id).unwrap_or(0),
        beats.cancel(id),
        beats.pending(),
    ));

    let mut watch = time::create_stopwatch();
    watch.start();
    watch.advance(120);
    watch.stop();
    watch.advance(1_000);
    lines.push(format!(
        "stopwatch: ran 120 ticks then stopped for 1000 | elapsed {} ticks = {} ms = {} s (held while stopped)",
        watch.elapsed(),
        watch.elapsed_millis(),
        watch.elapsed_seconds(),
    ));

    let mut once = time::create_timer(50);
    let before = once.advance(20);
    let fire = once.advance(40);
    lines.push(format!(
        "timer: one-shot 50 ticks | advance 20 fired {} | advance 40 fired {} | done {} fired {} | rate {} Hz tick {:?}",
        before,
        fire,
        once.is_done(),
        once.fired(),
        time::get_tick_rate(),
        time::tick_time(),
    ));

    lines.push(format!(
        "format: duration 3723000 -> {:?} | time_of_day 18500 -> {:?}",
        3_723_000,
        time::format_time_of_day(18_500),
    ));
    lines.push(format!(
        "format: duration {} -> {:?} | day {} ms -> {:?} | 1000 ticks {} ms -> {:?}",
        3_723_000,
        time::format_duration(3_723_000),
        time::DAY_MILLIS,
        time::format_duration(time::DAY_MILLIS),
        time::ticks_to_millis(1_000),
        time::format_duration(time::ticks_to_millis(1_000)),
    ));

    lines.push(format!(
        "host: now={} ms timestamp={} s timezone={} monotonic={} ms uptime={} s alpha={:.3} tick_count={:?} frames={}",
        time::now(),
        time::get_timestamp(),
        time::get_timezone(),
        time::monotonic_now(),
        time::get_uptime(),
        time::interpolation_alpha(),
        time::get_tick_count(),
        time::get_frame_count(),
    ));

    lines.push(format!(
        "refusals: {:?} | world_time={:?} day_time={:?} tick_count={:?}",
        time::refusals()
            .into_iter()
            .map(|(what, _)| what)
            .collect::<Vec<_>>(),
        time::world_time(),
        time::day_time(),
        time::get_tick_count(),
    ));

    lines
}
