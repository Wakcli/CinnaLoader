//! Section 56: a typed component store a mod owns.

use cinnaloader::config::ConfigValue;
use cinnaloader::data::{self, ComponentMap, ComponentType, Patch, ValueKind};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    data::register_component(ComponentType::new(
        "hello:durability",
        "Durability",
        ValueKind::Integer,
        ConfigValue::Integer(100),
    ));
    data::register_component(ComponentType::new(
        "hello:traits",
        "Traits",
        ValueKind::List,
        ConfigValue::List(Vec::new()),
    ));

    let mut map = ComponentMap::new("minecraft:iron_pickaxe");
    let set = data::set_component(&mut map, "hello:durability", ConfigValue::Integer(42));
    lines.push(format!("set integer: {set:?}"));
    map.insert_raw("hello:durability", ConfigValue::String("lots".to_owned()));
    lines.push(format!(
        "typed read refused: {:?}",
        data::get_component(&map, "hello:durability")
    ));

    let original = ComponentMap::new("minecraft:iron_pickaxe");
    let mut patch = Patch::new();
    patch.set("hello:durability", ConfigValue::Integer(7));
    patch.remove("hello:traits");
    let patched = patch.applied(&original).expect("the patch applies");
    lines.push(format!(
        "patch: durability={:?} traits_present={} original_unchanged={}",
        data::get_component_raw(&patched, "hello:durability"),
        data::has_component(&patched, "hello:traits"),
        !data::has_component(&original, "hello:durability"),
    ));

    let mut left = ComponentMap::new("wisp");
    let _ = left.set("hello:durability", ConfigValue::Integer(1));
    let mut right = ComponentMap::new("wisp");
    let _ = right.set("hello:durability", ConfigValue::Integer(2));
    let _ = right.set("hello:traits", ConfigValue::List(Vec::new()));
    left.merge(&right);
    lines.push(format!(
        "merge right wins: durability={:?} traits_present={}",
        data::get_component_raw(&left, "hello:durability"),
        data::has_component(&left, "hello:traits"),
    ));

    let empty = ComponentMap::new("wisp");
    let mut defaulted = ComponentMap::new("wisp");
    let _ = defaulted.set("hello:durability", ConfigValue::Integer(100));
    lines.push(format!(
        "absent: has={} default={:?} | present-at-default: has={} equal={}",
        data::has_component(&empty, "hello:durability"),
        data::default_of("hello:durability"),
        data::has_component(&defaulted, "hello:durability"),
        empty == defaulted,
    ));

    let mut round = ComponentMap::new("minecraft:iron_pickaxe");
    let _ = round.set(
        "hello:traits",
        ConfigValue::List(vec![
            ConfigValue::List(vec![ConfigValue::Integer(1), ConfigValue::Integer(2)]),
            ConfigValue::String("nested".to_owned()),
        ]),
    );
    let value = data::serialize_component(&round);
    let back = data::deserialize_component(&value).expect("a map reads back");
    let empty_back = data::deserialize_component(&data::serialize_component(&ComponentMap::new("wisp")))
        .expect("an empty map reads back");
    lines.push(format!(
        "round trip through config values: nested_equal={} empty_equal={}",
        back == round,
        empty_back == ComponentMap::new("wisp"),
    ));

    lines.push(format!(
        "refused: live_component={:?} live_stack={:?} write={}",
        data::get_live_component("minecraft:stone", "minecraft:damage"),
        data::get_stack_component("minecraft:damage"),
        data::set_live_component("wisp", "hello:glow", ConfigValue::Integer(1)),
    ));
    lines.push(format!(
        "refusals: {:?}",
        data::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
    ));

    data::unregister_component("hello:durability");
    data::unregister_component("hello:traits");
    lines
}
