//! Section 102 demo.

use std::collections::BTreeMap;

use cinnaloader::config::{ConfigType, ConfigValue};
use cinnaloader::protocol::{self, Compatibility, MapKind, Mapping, ProtocolError, Rule, Transform};

fn packet(fields: &[(&str, ConfigValue)]) -> ConfigValue {
    let mut map = BTreeMap::new();
    for (key, value) in fields {
        map.insert((*key).to_owned(), value.clone());
    }
    ConfigValue::Map(map)
}

fn bytes(value: &ConfigValue) -> Vec<u8> {
    value.render().into_bytes()
}

fn field(value: &ConfigValue, key: &str) -> Option<ConfigValue> {
    value.as_map()?.get(key).cloned()
}

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    protocol::clear();
    protocol::register_version(765, "1.20.4").expect("a free protocol");
    protocol::register_version(766, "1.20.5").expect("a free protocol");
    protocol::register_version(767, "1.21.0").expect("a free protocol");
    protocol::set_version(767);

    lines.push(format!(
        "cinnaloader::protocol registry 765={:?} 766={:?} supported={:?} current={} unknown={:?}",
        protocol::get_game_version(765).map(|version| version.to_string()),
        protocol::get_game_version(766).map(|version| version.to_string()),
        protocol::get_supported_versions()
            .into_iter()
            .map(|(protocol, version)| (protocol, version.to_string()))
            .collect::<Vec<_>>(),
        protocol::get_version(),
        protocol::get_game_version(999),
    ));

    protocol::register_translator(
        Rule::new(765, 766, "demo:block", "rename marker")
            .rename("old", "mid")
            .add("region", ConfigValue::Integer(0)),
    );
    protocol::register_translator(
        Rule::new(766, 767, "demo:block", "settle the name")
            .rename("mid", "name")
            .remove("temp")
            .retype("count", ConfigType::String),
    );

    let data = packet(&[
        ("old", ConfigValue::Integer(7)),
        ("temp", ConfigValue::Integer(9)),
        ("count", ConfigValue::Integer(5)),
    ]);
    let before = bytes(&data);
    let preview = protocol::dry_run("demo:block", &data, 765, 767).expect("a chain");
    lines.push(format!(
        "cinnaloader::protocol dry_run identical={} applied={:?} changes={:?}",
        bytes(&data) == before,
        preview.applied,
        preview
            .changes
            .iter()
            .map(|change| (change.rule.clone(), change.fields.clone()))
            .collect::<Vec<_>>(),
    ));

    let translated = protocol::translate_packet("demo:block", &data, 765, 767).expect("a chain");
    lines.push(format!(
        "cinnaloader::protocol translate name={:?} region={:?} count={:?} old_gone={} mid_gone={} temp_gone={}",
        field(&translated, "name"),
        field(&translated, "region"),
        field(&translated, "count"),
        field(&translated, "old").is_none(),
        field(&translated, "mid").is_none(),
        field(&translated, "temp").is_none(),
    ));

    lines.push(format!(
        "cinnaloader::protocol report applied={:?} json={}",
        protocol::get_translation_report().expect("a report").applied,
        protocol::get_translation_report().expect("a report").json(),
    ));

    let two_step_only = protocol::translate_packet("demo:block", &data, 765, 766).expect("one step");
    lines.push(format!(
        "cinnaloader::protocol two_step={:?} one_step_mid={:?} one_step_name={:?}",
        field(&translated, "name"),
        field(&two_step_only, "mid"),
        field(&two_step_only, "name"),
    ));

    protocol::register_translator(Rule::new(765, 766, "demo:gapped", "gapped first").rename("a", "b"));
    let gap = protocol::translate_packet("demo:gapped", &packet(&[("a", ConfigValue::Integer(1))]), 765, 767);
    lines.push(format!(
        "cinnaloader::protocol gap={} error={:?}",
        gap == Err(ProtocolError::MissingTranslation {
            from: 766,
            to: 767,
            packet: "demo:gapped".to_owned(),
        }),
        gap,
    ));

    protocol::register_mapping(Mapping::new(MapKind::Item, 765, 766, "demo:old_gem", "demo:gem"));
    lines.push(format!(
        "cinnaloader::protocol item demo:old_gem->{:?} missing={:?} compat={}",
        protocol::translate_item("demo:old_gem", 765, 766),
        protocol::translate_item("demo:absent", 765, 766),
        protocol::get_compatibility_status(765, 766),
    ));
    assert_eq!(protocol::get_compatibility_status(765, 766), Compatibility::Translatable);

    lines.push(format!(
        "cinnaloader::protocol refusal {:?} intercept={} read={:?} rewrite={}",
        protocol::why_unknown("rewrite"),
        protocol::intercept("demo:block"),
        protocol::read_traffic(),
        protocol::rewrite_traffic("demo:block", &translated),
    ));

    let rule = Rule::new(1, 2, "demo:x", "reversible")
        .rename("a", "b")
        .reverse(Transform::Rename {
            from: "b".to_owned(),
            to: "a".to_owned(),
        });
    lines.push(format!(
        "cinnaloader::protocol orientation reversible={} one_way_default={}",
        rule.reversible(),
        !Rule::new(1, 2, "demo:y", "one way").rename("a", "b").reversible(),
    ));

    lines
}
