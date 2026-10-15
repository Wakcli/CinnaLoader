//! Section 98 demo: consent gates recording, a redacted field is gone before it is stored,
//! the buffer drops the oldest at its cap, the aggregate carries names and no values, the
//! preview has a real byte size, and sending is refused with a sentence.

pub fn run() -> Vec<String> {
    use cinnaloader::config::ConfigValue;
    use cinnaloader::telemetry::{self, Configuration, Consent};

    let mut out = Vec::new();
    telemetry::clear();
    telemetry::reset_consent();
    telemetry::set_configuration(Configuration {
        cap: 3,
        redact: vec!["player".to_owned()],
    });

    let field = ("phase", ConfigValue::Text("load".to_owned()));
    let unset = telemetry::record_event("boot", &[field]);
    let unset_size = telemetry::get_queue_size();
    telemetry::deny();
    let field = ("phase", ConfigValue::Text("load".to_owned()));
    let denied = telemetry::record_event("boot", &[field]);
    let denied_size = telemetry::get_queue_size();
    assert!(!unset && !denied);
    assert_eq!(unset_size, 0);
    assert_eq!(denied_size, 0);
    out.push(format!(
        "consent(98): unset_refused={} denied_refused={} stored={} note={}",
        !unset,
        !denied,
        denied_size,
        telemetry::get_consent().describe()
    ));

    assert_eq!(telemetry::grant(), Consent::Denied);
    assert!(telemetry::record_event(
        "move",
        &[
            ("player", ConfigValue::Text("Alice".to_owned())),
            ("x", ConfigValue::Integer(3)),
            ("health", ConfigValue::Integer(20)),
        ]
    ));
    let event = telemetry::get_events().into_iter().next().expect("one event");
    assert_eq!(event.field("player"), None);
    assert_eq!(event.field("health"), Some(&ConfigValue::Integer(20)));
    out.push(format!(
        "redact(98): fields={} player_present={} x_present={} health={}",
        event.fields.len(),
        event.field("player").is_some(),
        event.field("x").is_some(),
        event.field("health").is_some()
    ));

    telemetry::clear();
    for name in ["one", "two", "three", "four"] {
        assert!(telemetry::record_event(name, &[("n", ConfigValue::Integer(1))]));
    }
    let kept: Vec<String> = telemetry::get_events()
        .into_iter()
        .map(|event| event.name)
        .collect();
    assert_eq!(kept, ["two", "three", "four"]);
    out.push(format!(
        "buffer(98): cap={} kept={} size={}",
        telemetry::get_configuration().cap,
        kept.join(","),
        telemetry::get_queue_size()
    ));

    telemetry::clear();
    assert!(telemetry::record_metric("fps", ConfigValue::Integer(60)));
    assert!(telemetry::record_error("io", "disk is full"));
    assert!(telemetry::record_performance("load", 42));
    let aggregate = telemetry::aggregate();
    let rendered = aggregate.render();
    assert!(!rendered.contains("disk is full"));
    assert!(!rendered.contains("60"));
    out.push(format!(
        "aggregate(98): total={} events={} fields={} carries_value={}",
        aggregate.total,
        aggregate.events.len(),
        aggregate.fields.len(),
        rendered.contains("disk is full") || rendered.contains("60")
    ));

    let preview = telemetry::preview();
    let exported = telemetry::export_local();
    assert_eq!(preview.text.as_bytes(), exported.as_slice());
    assert_eq!(preview.bytes, exported.len());
    out.push(format!(
        "preview(98): bytes={} head={}",
        preview.bytes,
        preview.text.chars().take(64).collect::<String>()
    ));

    let refusal = telemetry::flush().expect_err("this host does not send");
    assert!(refusal.contains("does not transmit"));
    out.push(format!(
        "refuse(98): flush_refused=true sentence={} listed={:?}",
        refusal,
        telemetry::refusals()
            .iter()
            .map(|(what, _)| what.clone())
            .collect::<Vec<_>>()
    ));

    telemetry::reset_consent();
    out
}
