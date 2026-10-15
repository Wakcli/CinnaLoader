//! Section 81: an error with a code, a chain of causes, a retry policy and a recovery action,
//! with the client's own error state refused.

use cinnaloader::errors::{self, Recovery, RecoveryStrategy, RetryPolicy};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    let step = errors::create("serialization", "the tag was malformed");
    let step = errors::context(step, "while reading chunk (3, 64, -1)");
    let step = errors::wrap("mod_load", "hello-mod did not load", step);
    let step = errors::context(step, "at startup");
    for (at, line) in step.render().lines().enumerate() {
        lines.push(format!("chain[{at}]: {line}"));
    }
    let root = errors::get_root_cause(&step);
    let order = step
        .causes()
        .iter()
        .map(|cause| cause.code.as_str())
        .collect::<Vec<_>>()
        .join(" -> ");
    lines.push(format!(
        "root cause {}: '{}' of {order} | origin {} recoverable {}",
        root.code,
        root.message,
        errors::classify(&step),
        step.is_recoverable(),
    ));

    match errors::describe_code("no_such_code") {
        Ok(code) => lines.push(format!("unknown code: unexpectedly known as {}", code.id)),
        Err(error) => lines.push(format!(
            "unknown code refused: {} ('{}')",
            error.code(),
            error.message(),
        )),
    }

    let retryable = errors::create("network", "the socket dropped");
    let fixed = RetryPolicy::fixed(3, 100);
    for attempt in 1..=4 {
        lines.push(format!(
            "fixed attempt {attempt}: retry={} delay={}ms",
            fixed.should_retry(attempt, &retryable),
            fixed.delay(attempt),
        ));
    }
    let exponential = RetryPolicy::exponential(5, 50, 200);
    for attempt in 1..=5 {
        lines.push(format!(
            "exponential attempt {attempt}: retry={} delay={}ms",
            exponential.should_retry(attempt, &retryable),
            exponential.delay(attempt),
        ));
    }

    let denied = errors::create("permission_denied", "the player said no");
    let verdicts = (1..=3)
        .map(|attempt| format!("{attempt}={}", exponential.should_retry(attempt, &denied)))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!(
        "non-recoverable recoverable={} retryable={} never retried: {verdicts}",
        denied.is_recoverable(),
        denied.is_retryable(),
    ));

    let _ = errors::register_recovery_strategy(RecoveryStrategy::new(
        "network",
        Recovery::UseDefault("cached response".to_owned()),
        RetryPolicy::fixed(2, 25),
    ));
    let action = errors::attempt_recovery(&retryable);
    let status = errors::get_recovery_status("network");
    lines.push(format!(
        "recovery for network: action {action} | attempts {:?} exhausted {:?}",
        status.as_ref().map(|held| held.attempts),
        status.as_ref().map(|held| held.exhausted),
    ));

    let report = errors::report();
    let groups = report
        .groups
        .iter()
        .map(|group| format!("{}={} ({}, {})", group.code, group.count, group.severity, group.origin))
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(format!(
        "report: total={} codes={} | {groups}",
        report.total,
        report.groups.len(),
    ));
    lines.push(format!("report text first line: {}", report.text().lines().next().unwrap_or("")));

    let refused = errors::set_error_state("broken");
    lines.push(format!(
        "refusal sentence: {}",
        errors::why_unknown("set_error_state"),
    ));
    lines.push(format!(
        "set_error_state refused as {} | {} entries listed",
        refused.err().map(|error| error.code().to_owned()).unwrap_or_default(),
        errors::refusals().len(),
    ));

    lines
}
