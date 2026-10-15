//! Section 74 demo: descriptors and values a mod owns, and the client's internals refused.

use std::collections::BTreeMap;

use cinnaloader::accessors::{self, AccessForm, Accessor, ValueStore};
use cinnaloader::config::{ConfigType, ConfigValue};
use cinnaloader::reflection::{self, Invoker, TypeDescriptor};

fn sum_args(args: &[ConfigValue]) -> ConfigValue {
    ConfigValue::Integer(args.iter().filter_map(ConfigValue::as_integer).sum::<i64>())
}

pub fn run() -> Vec<String> {
    let mut out = Vec::new();

    reflection::register(
        TypeDescriptor::new("hello:point")
            .described("a point this mod owns")
            .extends("hello:shape")
            .constructor(2)
            .field("x", ConfigValue::Integer(0))
            .field("y", ConfigValue::Integer(0))
            .read_only_field("label", ConfigValue::Text("origin".into()))
            .method("move_by", 2, ConfigType::Integer),
    );
    out.push(format!(
        "cinnaloader::reflection::get_type(\"hello:point\") fields={} methods={} constructor={:?} list_fields={:?} list_methods={:?}",
        reflection::get_type("hello:point").map(|d| d.fields.len()).unwrap_or(0),
        reflection::get_type("hello:point").map(|d| d.methods.len()).unwrap_or(0),
        reflection::get_constructor("hello:point"),
        reflection::list_fields("hello:point"),
        reflection::list_methods("hello:point"),
    ));

    let mut point = reflection::create_instance("hello:point").expect("hello:point is registered");
    let before = reflection::get_value(&point, "x");
    reflection::set_value(&mut point, "x", ConfigValue::Integer(5)).expect("x is writable");
    let after = reflection::get_value(&point, "x");
    out.push(format!(
        "cinnaloader::reflection::set_value x 0 -> 5: before={before:?} after={after:?} get_value={after:?} has_x={}",
        point.has("x"),
    ));

    out.push(format!(
        "cinnaloader::reflection::set_value unknown field \"z\": {:?}",
        reflection::set_value(&mut point, "z", ConfigValue::Integer(1)),
    ));
    out.push(format!(
        "cinnaloader::reflection::set_value wrong kind on \"x\": {:?}",
        reflection::set_value(&mut point, "x", ConfigValue::Text("five".into())),
    ));
    out.push(format!(
        "cinnaloader::reflection::set_value read-only \"label\": {:?}",
        reflection::set_value(&mut point, "label", ConfigValue::Text("moved".into())),
    ));

    reflection::register_invoker(Invoker::new("hello:sum", "hello:point", "move_by", sum_args));
    out.push(format!(
        "cinnaloader::reflection::invoke move_by(3, 4) = {:?}",
        reflection::invoke(
            "hello:point",
            "move_by",
            &[ConfigValue::Integer(3), ConfigValue::Integer(4)]
        ),
    ));
    out.push(format!(
        "cinnaloader::reflection::invoke move_by(3) wrong arity: {:?}",
        reflection::invoke("hello:point", "move_by", &[ConfigValue::Integer(3)]),
    ));

    out.push("cinnaloader::reflection::describe(\"hello:point\"):".to_owned());
    for line in reflection::describe("hello:point").unwrap_or_default().lines() {
        out.push(format!("  {line}"));
    }
    out.push(format!(
        "cinnaloader::reflection::resolve_symbol(\"hello:point.x\")={:?} find_symbol(\"label\")={:?}",
        reflection::resolve_symbol("hello:point.x").map(|s| s.name()),
        reflection::find_symbol("label").iter().map(|s| s.name()).collect::<Vec<_>>(),
    ));

    let mut raw = BTreeMap::new();
    raw.insert("name".to_owned(), ConfigValue::Text("wisp".to_owned()));
    let store = ValueStore::new(ConfigValue::Map(raw));
    accessors::register(Accessor::new(
        "hello:name",
        "hello:map",
        AccessForm::key("name"),
        ConfigType::Integer,
    ));
    out.push(format!(
        "cinnaloader::accessors::get(\"hello:name\") checked={:?} unchecked={:?}",
        accessors::get(&store, "hello:name"),
        accessors::get_unchecked(&store, "hello:name"),
    ));
    out.push(format!(
        "cinnaloader::accessors::get_errors()={:?}",
        accessors::get_errors(),
    ));

    for (what, why) in reflection::refusals() {
        out.push(format!("cinnaloader::reflection refusal {what}: {why}"));
    }
    for (what, why) in accessors::refusals() {
        out.push(format!("cinnaloader::accessors refusal {what}: {why}"));
    }

    reflection::unregister("hello:point");
    accessors::unregister("hello:name");
    out
}
