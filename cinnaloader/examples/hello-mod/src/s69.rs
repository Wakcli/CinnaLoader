//! Section 69 demo: the debug and developer tools a mod drives itself.

pub fn run() -> Vec<String> {
    use cinnaloader::debug;
    let mut out = Vec::new();

    out.push(format!(
        "debug(69): enable={} define trace_calls={} set(true)={} set(true) again={} changed={:?}",
        debug::enable(),
        debug::define_flag("trace_calls", "perf", false),
        debug::set_flag("trace_calls", true),
        debug::set_flag("trace_calls", true),
        debug::changed_flags(),
    ));

    let coords = debug::set_readout("coords", "XYZ", "10 64 -3", 0);
    let fps = debug::set_readout_unit("fps", "FPS", "120", "fps", 1);
    let tps = debug::set_readout_unit("tps", "TPS", "20.0", "tps", 2);
    out.push(format!(
        "debug(69): readouts new={coords},{fps},{tps} exact order={:?}",
        debug::render_readouts()
    ));

    debug::watch("tick_ms", 10.0);
    debug::watch("tick_ms", 30.0);
    let stats = debug::watch("tick_ms", 20.0);
    out.push(format!(
        "debug(69): watch tick_ms last={} min={} max={} average={} changed={}",
        stats.last, stats.min, stats.max, stats.average, stats.changed
    ));

    debug::register_command("sum", |args: &[String]| {
        args.iter()
            .filter_map(|arg| arg.parse::<i64>().ok())
            .sum::<i64>()
            .to_string()
    });
    out.push(format!(
        "debug(69): console 'sum 4 5 6' -> {:?} 'nosuch' -> {:?}",
        debug::dispatch("sum 4 5 6"),
        debug::dispatch("nosuch"),
    ));

    let before = debug::snapshot();
    out.push(format!(
        "debug(69): snapshot round trip parsed={} bytes={}",
        debug::load_snapshot(&before).is_ok(),
        before.len(),
    ));
    debug::set_flag("trace_calls", false);
    let after = debug::snapshot();
    out.push(format!(
        "debug(69): diff one changed key={:?}",
        debug::diff_snapshots(&before, &after)
    ));

    debug::register_invariant("has_id", || Err("the id is empty".to_owned()));
    debug::register_invariant("size_ok", || Ok(()));
    out.push(format!(
        "debug(69): check_all failed={} failures={:?}",
        debug::check_all(),
        debug::failures()
    ));

    out
}

