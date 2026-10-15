//! Section 67 demo: real metric aggregation, caller-timed sections and frames, and the
//! client's own profiling refused.

use std::time::Duration;

use cinnaloader::metrics;
use cinnaloader::profiler;

pub fn run() -> Vec<String> {
    let mut out = Vec::new();

    metrics::reset();
    profiler::reset();

    metrics::register_counter("s67.requests");
    metrics::increment("s67.requests", 5);
    metrics::increment("s67.requests", 2);
    out.push(format!(
        "metrics counter::get(\"s67.requests\") = {:?} (expected 7)",
        metrics::get("s67.requests").and_then(|m| m.counter())
    ));

    metrics::register_gauge("s67.workers");
    metrics::set("s67.workers", 4);
    metrics::increment("s67.workers", 2);
    metrics::decrement("s67.workers", 3);
    out.push(format!(
        "metrics gauge::get(\"s67.workers\") = {:?} (expected 3)",
        metrics::get("s67.workers").and_then(|m| m.gauge())
    ));

    metrics::register_histogram("s67.frame_ms");
    for value in [10.0, 20.0, 30.0, 40.0] {
        metrics::observe("s67.frame_ms", value);
    }
    let stats = metrics::get("s67.frame_ms")
        .and_then(|m| m.histogram())
        .expect("s67.frame_ms is a histogram");
    out.push(format!(
        "metrics histogram count={} min={} max={} mean={} p50={} p90={} p99={} (expected 4 10 40 25 20 40 40)",
        stats.count, stats.min, stats.max, stats.mean, stats.p50, stats.p90, stats.p99
    ));

    metrics::register_counter("s67.bytes");
    metrics::increment("s67.bytes", 600);
    metrics::mark("s67.bytes");
    metrics::increment("s67.bytes", 150);
    out.push(format!(
        "metrics rate over 3s = {:?} (delta 150 -> 50.0/s)",
        metrics::rate("s67.bytes", Duration::from_secs(3))
    ));

    profiler::begin_section("s67.tick");
    profiler::begin_section("s67.ai");
    profiler::end_section(Duration::from_millis(4));
    profiler::end_section(Duration::from_millis(10));
    let tick = profiler::get_section("s67.tick").expect("s67.tick is a section");
    out.push(format!(
        "profiler section s67.tick total={}ms self={}ms (expected total 10 self 6, child s67.ai excluded)",
        tick.total.as_millis(),
        tick.self_time.as_millis()
    ));

    for millis in [10, 20, 30, 40] {
        profiler::record_frame(Duration::from_millis(millis));
    }
    out.push(format!(
        "profiler frames current={:?} mean={:?} max={:?} (expected 40 25 40 ms)",
        profiler::get_current_frame_time().map(|d| d.as_millis()),
        profiler::get_average().map(|d| d.as_millis()),
        profiler::get_maximum().map(|d| d.as_millis())
    ));

    profiler::set_slow_frame_threshold(Duration::from_millis(16));
    profiler::record_frame(Duration::from_millis(120));
    out.push(format!(
        "profiler slow frames={:?} at their exact durations (threshold 16ms)",
        profiler::get_slow_frames()
            .iter()
            .map(Duration::as_millis)
            .collect::<Vec<_>>()
    ));

    profiler::measure("s67.render", Duration::from_millis(5));
    profiler::measure("s67.physics", Duration::from_millis(9));
    let hotspots = profiler::get_hotspots();
    out.push(format!(
        "profiler mod cost={}ms hotspot={} self={}ms (expected cost 14, s67.physics 9ms)",
        profiler::get_mod_cost().as_millis(),
        hotspots[0].name,
        hotspots[0].self_time.as_millis()
    ));

    out.push(format!(
        "metrics export_json contains histogram p99=40: {}",
        metrics::export_json().contains("\"p99\":40")
    ));

    for (what, why) in profiler::refusals() {
        out.push(format!("refused {what}: {why}"));
    }

    out
}
