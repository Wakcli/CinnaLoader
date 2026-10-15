//! Section 60 demo.

use std::sync::{Arc, Mutex};

use cinnaloader::config::{ConfigType, ConfigValue};
use cinnaloader::events::Payload;
use cinnaloader::game_rules::{self, GameRule};
use cinnaloader::gameplay::{self, Phase, RuleProfile};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    game_rules::register(
        GameRule::new("hello:speed", ConfigType::Integer, ConfigValue::Integer(3))
            .name("Speed")
            .description("how fast the thing moves")
            .bounded(0.0, 10.0),
    );
    lines.push(format!(
        "game_rules::register hello:speed default={} name={:?} types={}",
        game_rules::get_default("hello:speed").map_or_else(|| "?".to_owned(), |value| value.render()),
        game_rules::get("hello:speed").map(|rule| rule.name),
        game_rules::known_types().len(),
    ));

    let wrong = game_rules::set_value("hello:speed", ConfigValue::Text("fast".to_owned()));
    lines.push(format!("set_value wrong kind -> {wrong:?}"));

    let out = game_rules::set_value("hello:speed", ConfigValue::Integer(99));
    lines.push(format!("set_value out of range -> {out:?}"));

    let heard = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&heard);
    game_rules::register_change_handler(move |change| {
        into.lock().unwrap().push(format!(
            "{} {}->{}",
            change.rule,
            change.old.render(),
            change.new.render()
        ));
    });
    game_rules::set_value("hello:speed", ConfigValue::Integer(7)).ok();
    let fired = heard.lock().unwrap().len();
    game_rules::set_value("hello:speed", ConfigValue::Integer(7)).ok();
    let again = heard.lock().unwrap().len() - fired;
    lines.push(format!(
        "change event {:?}; setting the same value fired {again} more",
        heard.lock().unwrap()
    ));

    let reset = game_rules::reset("hello:speed");
    lines.push(format!(
        "reset -> {reset:?} now={}",
        game_rules::get_value("hello:speed").map_or_else(|| "?".to_owned(), |value| value.render())
    ));

    lines.push(format!(
        "config round trip export={:?} import={:?}",
        game_rules::export_value("hello:speed"),
        game_rules::import_value("hello:speed", "9"),
    ));

    let order = Arc::new(Mutex::new(Vec::new()));
    for (name, priority, phase) in [
        ("first", 10, Phase::Before),
        ("second", 5, Phase::Before),
        ("last", 1, Phase::After),
    ] {
        let order = Arc::clone(&order);
        gameplay::register_damage_rule(name, priority, phase, move |_| {
            order.lock().unwrap().push(name);
        });
    }
    let mut payload = Payload::Value(4.0);
    let ran = gameplay::dispatch("damage", &mut payload);
    lines.push(format!(
        "dispatch damage ran={ran} order={:?} payload={payload:?}",
        order.lock().unwrap()
    ));

    game_rules::register(GameRule::new(
        "hello:keep_inventory",
        ConfigType::Boolean,
        ConfigValue::Boolean(false),
    ));
    gameplay::register_profile(
        RuleProfile::new("hard")
            .set("hello:speed", ConfigValue::Integer(9))
            .set("hello:keep_inventory", ConfigValue::Boolean(true)),
    );
    let changed = gameplay::apply_profile("hard");
    lines.push(format!(
        "profile 'hard' changed {} rule(s): {}",
        changed.len(),
        changed.iter().map(|change| change.rule.clone()).collect::<Vec<_>>().join(", ")
    ));

    lines.push(format!(
        "game_rules::refusals {:?}",
        game_rules::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>()
    ));
    lines.push(format!(
        "game_rules::client_rules={:?} set_client_rule={}",
        game_rules::client_rules(),
        game_rules::set_client_rule("hello:speed", ConfigValue::Integer(1))
    ));
    lines.push(format!(
        "gameplay::refusals {:?} client_hooks={} run_client_hook={}",
        gameplay::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
        gameplay::client_hooks().len(),
        gameplay::run_client_hook("damage")
    ));

    lines
}
