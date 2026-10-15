//! Section 99 demo: save states and snapshots.

use cinnaloader::config::ConfigValue;
use cinnaloader::snapshots;

use std::collections::BTreeMap;

fn map(pairs: &[(&str, ConfigValue)]) -> ConfigValue {
    ConfigValue::Map(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn field(state: &ConfigValue, key: &str) -> String {
    state
        .as_map()
        .and_then(|fields| fields.get(key))
        .map(ConfigValue::render)
        .unwrap_or_else(|| "?".to_owned())
}

fn describe(entry: &snapshots::DiffEntry) -> String {
    format!(
        "{} {} -> {}",
        entry.path,
        entry.before.as_ref().map(ConfigValue::render).unwrap_or_else(|| "-".to_owned()),
        entry.after.as_ref().map(ConfigValue::render).unwrap_or_else(|| "-".to_owned()),
    )
}

pub fn run() -> Vec<String> {
    let mut out = Vec::new();
    snapshots::clear();
    snapshots::set_capacity(4);

    let captured = map(&[
        ("hp", ConfigValue::Integer(20)),
        ("name", ConfigValue::Text("wisp".to_owned())),
    ]);
    let snap = snapshots::create("before", "full health", 1_700_000_000, &captured);
    out.push(format!(
        "captured {:?} size={} checksum={:#010x}",
        snap.name,
        snapshots::get_size(&snap),
        snap.checksum,
    ));

    let mut state = map(&[
        ("hp", ConfigValue::Integer(3)),
        ("name", ConfigValue::Text("wisp".to_owned())),
    ]);
    out.push(format!("changed hp 20 -> {}", field(&state, "hp")));
    match snapshots::restore("before", &mut state) {
        Ok(()) => out.push(format!("restored hp back to {} (the captured value)", field(&state, "hp"))),
        Err(error) => out.push(format!("restore failed: {error}")),
    }

    let mut broken = snapshots::get("before").expect("just captured");
    broken.bytes[3] ^= 0xff;
    snapshots::insert(broken);
    match snapshots::restore("before", &mut state) {
        Err(error) => out.push(format!("tampered bytes refused: {error}")),
        Ok(()) => out.push("tampered bytes restored (wrong)".to_owned()),
    }
    match snapshots::restore("absent", &mut state) {
        Err(error) => out.push(format!("unknown snapshot refused: {error}")),
        Ok(()) => out.push("unknown snapshot restored (wrong)".to_owned()),
    }
    snapshots::delete("before");
    snapshots::create("before", "full health", 1_700_000_000, &captured);

    let left = snapshots::create(
        "left",
        "before",
        1,
        &map(&[("a", ConfigValue::Integer(1)), ("b", ConfigValue::Integer(2))]),
    );
    let right = snapshots::create(
        "right",
        "after",
        2,
        &map(&[("a", ConfigValue::Integer(3)), ("c", ConfigValue::Integer(4))]),
    );
    let diff = snapshots::create_diff(&left, &right);
    out.push(format!(
        "diff added [{}] removed [{}] changed [{}]",
        diff.added.iter().map(describe).collect::<Vec<_>>().join(", "),
        diff.removed.iter().map(describe).collect::<Vec<_>>().join(", "),
        diff.changed.iter().map(describe).collect::<Vec<_>>().join(", "),
    ));
    let mut replayed = left.state.clone();
    let moved = snapshots::apply_diff(&mut replayed, &diff);
    out.push(format!("applied {moved} paths, matches right: {}", replayed == right.state));

    let one = snapshots::create("one", "same", 3, &map(&[("a", ConfigValue::Integer(1))]));
    let two = snapshots::create("two", "same", 4, &map(&[("a", ConfigValue::Integer(1))]));
    let same = snapshots::compare(&one, &two);
    out.push(format!(
        "identical snapshots: diff empty={} equal={} changed_paths={}",
        snapshots::create_diff(&one, &two).is_empty(),
        same.equal,
        same.changed_paths,
    ));

    let bytes = snapshots::export("before").expect("a captured snapshot");
    let back = snapshots::import(&bytes).expect("a valid export");
    out.push(format!(
        "export/import {}-byte round trip, bytes equal={} state equal={}",
        bytes.len(),
        back.bytes == snap.bytes,
        snapshots::compare(&snap, &back).equal,
    ));

    snapshots::set_capacity(2);
    snapshots::create("r1", "", 1, &ConfigValue::Integer(1));
    snapshots::create("r2", "", 2, &ConfigValue::Integer(2));
    snapshots::create("r3", "", 3, &ConfigValue::Integer(3));
    out.push(format!(
        "ring cap=2 holds {:?}, oldest dropped={}",
        snapshots::get_all().iter().map(|held| held.name.clone()).collect::<Vec<_>>(),
        snapshots::get("r1").is_none(),
    ));

    out.push(format!(
        "cannot capture: {:?}",
        snapshots::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
    ));
    snapshots::clear();
    out
}
