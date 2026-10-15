//! Section 65 demo.

pub fn run() -> Vec<String> {
    use cinnaloader::config::{ConfigType, ConfigValue};
    use cinnaloader::settings::{self, Setting};

    let mut lines = Vec::new();

    let first = settings::register(
        Setting::new("hello:draw_distance", ConfigType::Integer, ConfigValue::Integer(8))
            .label("Draw distance")
            .tooltip("chunks to draw")
            .category("hello:display")
            .bounded(2.0, 32.0),
    );
    let duplicate = settings::register(Setting::new(
        "hello:draw_distance",
        ConfigType::Integer,
        ConfigValue::Integer(4),
    ));
    lines.push(format!(
        "cinnaloader::settings::register first={first} duplicate={duplicate} -> duplicate refused = {}",
        !duplicate,
    ));

    let wrong = settings::set("hello:draw_distance", ConfigValue::Text("far".to_owned()));
    let range = settings::set("hello:draw_distance", ConfigValue::Integer(99));
    lines.push(format!(
        "cinnaloader::settings::set wrong kind = {:?}, out of range = {:?}",
        wrong.map_err(|error| error.to_string()),
        range.map_err(|error| error.to_string()),
    ));

    let heard = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = std::sync::Arc::clone(&heard);
    settings::watch("hello:draw_distance", move |change| {
        sink.lock().unwrap().push(format!(
            "{} {}->{}",
            change.setting,
            change.old.render(),
            change.new.render()
        ));
    });
    let change = settings::set("hello:draw_distance", ConfigValue::Integer(12)).unwrap();
    let same = settings::set("hello:draw_distance", ConfigValue::Integer(12)).unwrap();
    lines.push(format!(
        "cinnaloader::settings::set change = {:?} same-value = {:?} watched = {:?}",
        change
            .as_ref()
            .map(|change| format!("{}->{}", change.old.render(), change.new.render())),
        same.is_none(),
        *heard.lock().unwrap(),
    ));

    let back = settings::reset("hello:draw_distance").unwrap();
    lines.push(format!(
        "cinnaloader::settings::reset = {:?} now = {:?}",
        back.map(|change| format!("{}->{}", change.old.render(), change.new.render())),
        settings::get("hello:draw_distance"),
    ));

    let hits: Vec<String> = settings::search("draw")
        .iter()
        .map(|setting| setting.id.clone())
        .collect();
    lines.push(format!(
        "cinnaloader::settings::search(\"draw\") by id or label = {hits:?}",
    ));

    settings::set("hello:draw_distance", ConfigValue::Integer(16)).unwrap();
    let snapshot = settings::export();
    let _ = settings::reset_all();
    let imported = settings::import(&snapshot);
    lines.push(format!(
        "cinnaloader::settings::export/import applied = {:?} round trip equal = {}",
        imported.map_err(|error| error.to_string()),
        settings::export() == snapshot,
    ));

    lines.push(format!(
        "cinnaloader::settings::get_video = {:?} set_client = {} refusals = {}",
        settings::get_video(),
        settings::set_client("video", ConfigValue::Boolean(true)),
        settings::refusals().len(),
    ));
    lines.push(format!(
        "cinnaloader::settings::why_unknown(\"video\") = {}",
        settings::why_unknown("video"),
    ));

    lines
}
