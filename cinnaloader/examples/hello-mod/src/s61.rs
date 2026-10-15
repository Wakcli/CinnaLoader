//! Section 61 demo.

use cinnaloader::config::ConfigValue;
use cinnaloader::query::{self, Filter, Query, Record};
use std::ops::Not;

fn ids(records: Vec<Record>) -> Vec<String> {
    records.into_iter().map(|record| record.id).collect()
}

fn records() -> Vec<Record> {
    vec![
        Record::new("zombie-a", "entity", 1.0, 64.0, 0.0)
            .hostile(true)
            .health(18.0)
            .age(40)
            .tag("role", ConfigValue::Text("zombie".to_owned()))
            .number("speed", 0.23),
        Record::new("zombie-b", "entity", 4.0, 64.0, 0.0)
            .hostile(true)
            .health(20.0)
            .age(10)
            .tag("role", ConfigValue::Text("zombie".to_owned()))
            .number("speed", 0.23),
        Record::new("cow-a", "entity", 8.0, 64.0, 0.0)
            .health(10.0)
            .age(5)
            .tag("role", ConfigValue::Text("animal".to_owned()))
            .number("speed", 0.3),
        Record::new("chest-a", "block", 2.0, 64.0, 2.0).component("hello:container"),
    ]
}

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    query::clear();
    for record in records() {
        query::add(record);
    }

    let by_tag = query::entities()
        .by_tag_value("role", ConfigValue::Text("zombie".to_owned()))
        .sort_by_id();
    lines.push(format!(
        "cinnaloader::query::entities().by_tag_value(role=zombie) -> {:?}",
        ids(by_tag.collect())
    ));

    let composed = Query::new(records()).filter(
        Filter::Hostile
            .and(
                Filter::Radius {
                    origin: (0.0, 64.0, 0.0),
                    radius: 2.0,
                }
                .not(),
            )
            .or(Filter::Component("hello:container".to_owned())),
    );
    lines.push(format!(
        "cinnaloader::query::Filter and/or/not (hostile and not near 2, or container) -> {:?}",
        ids(composed.collect())
    ));

    let sorted = Query::new(records())
        .by_type("entity")
        .sort_by_value("speed", true);
    lines.push(format!(
        "cinnaloader::query::Query::sort_by_value(speed, desc) tie-break id asc -> {:?}",
        ids(sorted.collect())
    ));

    let paged = Query::new(records()).by_type("entity").sort_by_id().limit(2);
    lines.push(format!(
        "cinnaloader::query::Query::limit(2): count before paging={} slice={:?}",
        paged.count(),
        ids(paged.collect())
    ));

    let past_end = Query::new(records()).by_type("entity").sort_by_id().offset(99);
    lines.push(format!(
        "cinnaloader::query::Query::offset(99) past the end -> {:?} (count={})",
        ids(past_end.collect()),
        past_end.count()
    ));

    let built = Query::new(records())
        .by_type("entity")
        .within_radius((0.0, 64.0, 0.0), 5.0)
        .sort_by_distance((0.0, 64.0, 0.0))
        .limit(2);
    let text = built.render();
    let parsed = Query::parse(&text).expect("the demo's own text form reads back");
    lines.push(format!(
        "cinnaloader::query::Query::parse(\"{text}\") -> {:?} == built {:?}",
        ids(parsed.collect()),
        ids(built.collect())
    ));

    match Query::parse("kind=zombie nonsense=1") {
        Ok(_) => lines.push("cinnaloader::query::Query::parse accepted a malformed query".to_owned()),
        Err(error) => lines.push(format!(
            "cinnaloader::query::Query::parse refused malformed at position {}: {}",
            error.position, error.message
        )),
    }

    lines.push(format!(
        "cinnaloader::query::live_entities()={:?} why_unknown={}",
        query::live_entities(),
        query::why_unknown("entities")
    ));

    lines
}
