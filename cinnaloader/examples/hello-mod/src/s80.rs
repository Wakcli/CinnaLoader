//! Section 80 demo: a two-step chain run in order, a missing step, a dry run, an id
//! deprecation, a refused future version and the client-data refusal sentence.

pub fn run() -> Vec<String> {
    use cinnaloader::migration::{self, Error, Kind, Step, Support};
    use cinnaloader::serialization::{Map, Value, to_binary};

    let mut out = Vec::new();
    migration::clear();
    migration::set_version(3);

    fn document(version: i64) -> Value {
        let mut map = Map::new();
        map.insert("version", Value::Int(version));
        map.insert("old_name", Value::Str("gem".to_owned()));
        Value::Map(map)
    }

    migration::register(Step::of(Kind::Config, 1, 2, "rename old_name").rename("old_name", "name"));
    migration::register(Step::of(Kind::Config, 2, 3, "stamp schema").set("schema", Value::Int(3)));

    let fresh = document(1);
    let before = to_binary(&fresh);
    let preview = migration::dry_run(Kind::Config, &fresh, 1, 3).expect("the chain exists");
    let names_path = preview.changes.iter().any(|change| change.path == "old_name");
    let unchanged = to_binary(&fresh) == before;
    assert!(names_path && unchanged, "a dry run reports paths without moving the document");
    out.push(format!(
        "migration(80): dry run applied={:?} names old_name={names_path} bytes unchanged={unchanged}",
        preview.applied
    ));

    let mut doc = document(1);
    let run = migration::run_all(Kind::Config, &mut doc, 1, 3).expect("both steps exist");
    let held = doc.as_map().expect("a map");
    assert_eq!(run.applied, ["rename old_name", "stamp schema"]);
    assert!(held.contains_key("name") && !held.contains_key("old_name"));
    assert_eq!(held.get("schema"), Some(&Value::Int(3)));
    out.push(format!(
        "migration(80): chain applied in order = {:?} keys={:?} schema={:?}",
        run.applied,
        held.keys().collect::<Vec<_>>(),
        held.get("schema")
    ));

    let missing = migration::run_all(Kind::Config, &mut document(1), 3, 4);
    assert_eq!(missing, Err(Error::MissingStep { from: 3, to: 4 }));
    out.push(format!(
        "migration(80): missing step refused = {:?}",
        missing.unwrap_err()
    ));

    migration::deprecate("s80:old", "s80:new");
    let mut older = Value::Map(Map::new());
    if let Value::Map(map) = &mut older {
        map.insert("version", Value::Int(3));
        map.insert("s80:old", Value::Int(7));
    }
    migration::run_all(Kind::Config, &mut older, 3, 3).expect("an empty chain still migrates");
    let older_map = older.as_map().expect("a map");
    let moved = older_map.get("s80:new").cloned();
    let old_gone = !older_map.contains_key("s80:old");
    assert_eq!(moved, Some(Value::Int(7)));
    assert!(old_gone);
    out.push(format!(
        "migration(80): deprecated s80:old -> s80:new moved={moved:?} old_gone={old_gone} renamed={:?}",
        migration::renamed("s80:old")
    ));

    let future = migration::check(4);
    assert_eq!(
        future,
        Err(Error::FromFuture {
            found: 4,
            supported: 3
        })
    );
    out.push(format!(
        "migration(80): future refused = {:?} older(2)={:?} supported(3)={:?}",
        future.unwrap_err(),
        migration::compatibility(2),
        migration::compatibility(3)
    ));

    let refused = migration::migrate_client("level");
    assert!(matches!(refused, Err(Error::Refused(_))));
    out.push(format!(
        "migration(80): client refusal = {}",
        migration::why_unknown("level")
    ));

    let kinds = migration::get_all().len();
    assert_eq!(kinds, 2);
    out.push(format!(
        "migration(80): registered={kinds} refusals={:?} support={:?}",
        migration::refusals()
            .into_iter()
            .map(|(what, _)| what)
            .collect::<Vec<_>>(),
        Support::Supported
    ));

    out
}
