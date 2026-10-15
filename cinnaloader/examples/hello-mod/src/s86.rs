//! Section 86 demo: safe mode and recovery startup.

use cinnaloader::{persistence, safe_mode};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    safe_mode::reset();
    safe_mode::set_policy(3, 1_000);

    let _ = safe_mode::record_crash("hello:loop", 0);
    let _ = safe_mode::record_crash("hello:loop", 10);
    lines.push(format!(
        "safe_mode: 2 crashes -> enabled={} count={} (N-1 must not trip)",
        safe_mode::is_enabled(),
        safe_mode::crash_count("hello:loop"),
    ));
    let _ = safe_mode::record_crash("hello:loop", 20);
    lines.push(format!(
        "safe_mode: 3 crashes -> enabled={} (N must trip) reason={:?}",
        safe_mode::is_enabled(),
        safe_mode::get_reason(),
    ));

    safe_mode::reset();
    safe_mode::set_policy(3, 1_000);
    let _ = safe_mode::record_crash("hello:aged", 0);
    let _ = safe_mode::record_crash("hello:aged", 0);
    let aged = safe_mode::record_crash("hello:aged", 5_000);
    lines.push(format!(
        "safe_mode: crash outside the 1000ms window -> count={aged} enabled={} (must not count)",
        safe_mode::is_enabled(),
    ));

    safe_mode::reset();
    safe_mode::set_policy(3, 60_000);
    let _ = safe_mode::record_crash("hello:bad", 0);
    let _ = safe_mode::record_crash("hello:bad", 1);
    let _ = safe_mode::record_crash("hello:bad", 2);
    lines.push(format!(
        "safe_mode: reason={:?} suspected={:?} previous_crash={:?}",
        safe_mode::get_reason(),
        safe_mode::get_suspected_mods(),
        safe_mode::get_previous_crash(),
    ));
    lines.push(format!(
        "safe_mode: can_start={} while the cause is enabled, disabled={:?}",
        safe_mode::can_start(),
        safe_mode::get_disabled_mods(),
    ));
    lines.push(format!(
        "safe_mode: disable_mod={} can_start={} disabled={:?}",
        safe_mode::disable_mod("hello:bad", "crashed 3 times"),
        safe_mode::can_start(),
        safe_mode::get_disabled_mods(),
    ));
    let plan = safe_mode::recovery_plan();
    lines.push(format!(
        "safe_mode: plan {} ordered action(s)",
        plan.len(),
    ));
    for step in &plan {
        lines.push(format!(
            "safe_mode: plan {} {} -> {}",
            step.order, step.action, step.detail,
        ));
    }

    let saved = safe_mode::save();
    lines.push(format!(
        "safe_mode: save={saved} store={:?}",
        persistence::current_store(),
    ));
    safe_mode::disable();
    lines.push(format!(
        "safe_mode: after in-memory disable -> enabled={}",
        safe_mode::is_enabled(),
    ));
    let reloaded = safe_mode::reload();
    lines.push(format!(
        "safe_mode: reload={reloaded} enabled={} reason={:?}",
        safe_mode::is_enabled(),
        safe_mode::get_reason(),
    ));

    let report = safe_mode::recovery_report();
    lines.push(format!("safe_mode: report {}", report.summary()));
    let text = safe_mode::create_recovery_report("text");
    let json = safe_mode::create_recovery_report("json");
    lines.push(format!(
        "safe_mode: text names mod={} reason={} | json safe_mode={} count3={}",
        text.contains("hello:bad"),
        text.contains("crashed 3 times"),
        json.contains("\"safe_mode\":true"),
        json.contains("\"count\":3"),
    ));
    lines.push(format!(
        "safe_mode: validate mods={:?} resources={:?}",
        safe_mode::validate_all_mods(),
        safe_mode::validate_all_resources(),
    ));

    let cleared = safe_mode::clear_safe_mode("the demo asked for a normal start");
    lines.push(format!(
        "safe_mode: clear={cleared} enabled={} cleared_reason={:?}",
        safe_mode::is_enabled(),
        safe_mode::cleared_reason(),
    ));
    let refused = safe_mode::refusals();
    lines.push(format!(
        "safe_mode: refusals={} client_safe_mode={} warn={:?}",
        refused.len(),
        safe_mode::known_only_to_the_client::is_in_client_safe_mode(),
        safe_mode::why_unknown("client_safe_mode"),
    ));

    lines
}
