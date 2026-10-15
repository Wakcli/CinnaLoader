//! Section 82 demo.

use cinnaloader::crash;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    crash::clear_history();
    lines.push(format!("crash: install panic hook -> {}", crash::install()));

    let caught = std::panic::catch_unwind(|| panic!("hello-mod s82 probe")).is_err();
    let last = crash::get_last_crash();
    lines.push(format!(
        "crash: caught {} | message {:?} | thread {} | mod {}",
        caught,
        last.as_ref().map(|crash| crash.message.clone()).unwrap_or_default(),
        last.as_ref().map(|crash| crash.thread.clone()).unwrap_or_default(),
        last.as_ref().map(|crash| crash.mod_id.clone()).unwrap_or_default(),
    ));

    crash::record_for("hello:demo", "the same failure");
    crash::record_for("hello:demo", "the same failure");
    crash::record_for("hello:demo", "a different failure");
    let report = crash::get_report();
    let same = report.groups.iter().find(|group| group.count == 2).map_or(0, |group| group.count);
    let other = report.groups.iter().find(|group| group.count == 1).map_or(0, |group| group.count);
    lines.push(format!(
        "crash: {} records, {} groups | same x{} different x{}",
        report.total,
        report.groups.len(),
        same,
        other
    ));

    crash::set_policy(3, 60_000);
    crash::record_for("hello:policy", "flaky");
    crash::record_for("hello:policy", "flaky");
    let before = crash::should_disable("hello:policy");
    crash::record_for("hello:policy", "flaky");
    let after = crash::should_disable("hello:policy");
    lines.push(format!(
        "crash: policy 3/60000ms | disable at 2 crashes {} at 3 {} | disabled {}",
        before,
        after,
        crash::disabled_reason("hello:policy").is_some()
    ));

    lines.push(format!("crash: report {}", crash::summary()));

    lines.push(format!(
        "crash: client crash log {:?} | {}",
        crash::known_only_to_the_client::read_crash_log(),
        crash::why_unknown("crash_reporter")
    ));

    lines
}
