//! Section 57 demo.

use std::time::Duration;

use cinnaloader::config::ConfigValue;
use cinnaloader::{fs, persistence};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    let previous = fs::root();
    let root = std::env::temp_dir().join(format!("cinnaloader-s57-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    fs::set_root(root.clone());

    lines.push(format!(
        "persistence: format={} refusals={:?}",
        persistence::FORMAT_VERSION,
        persistence::refusals()
            .into_iter()
            .map(|(what, _)| what)
            .collect::<Vec<_>>()
    ));

    let nested = ConfigValue::List(vec![
        ConfigValue::Float(1.5),
        ConfigValue::List(vec![
            ConfigValue::Text("deep".into()),
            ConfigValue::Integer(7),
        ]),
    ]);
    let id = persistence::open("demo").expect("a store of the mod's own");
    persistence::set("nested", nested.clone());
    persistence::set("ratio", ConfigValue::Float(0.25));
    let dirty_before = persistence::is_dirty();
    let written = persistence::flush().expect("the store writes");
    let dirty_after = persistence::is_dirty();
    let path = fs::resolve("demo.sav").expect("the store's file").display().to_string();
    lines.push(format!("persistence: open id={id} name=demo file={path}"));
    lines.push(format!(
        "persistence: save bytes={written} dirty_before={dirty_before} dirty_after={dirty_after}"
    ));
    persistence::close();

    persistence::open("demo").expect("the store reopens");
    lines.push(format!(
        "persistence: load nested_identical={} ratio_identical={} keys={:?}",
        persistence::get("nested") == Some(nested.clone()),
        persistence::get("ratio") == Some(ConfigValue::Float(0.25)),
        persistence::keys()
    ));
    persistence::close();

    persistence::open("auto").expect("an autosaving store");
    persistence::set_interval(Duration::from_millis(60));
    persistence::set("blocks", ConfigValue::Integer(1));
    let before = persistence::autosave_current().expect("a store is open");
    std::thread::sleep(Duration::from_millis(90));
    let after = persistence::autosave_current().expect("a store is open");
    lines.push(format!(
        "persistence: autosave not_before={before:?} fired_after={after:?} dirty_cleared={}",
        !persistence::is_dirty()
    ));
    persistence::close();

    let _ = fs::write_text(
        "future.sav",
        "# cinnaloader persistence 'future' format=2 schema=0\n",
    );
    let refused = match persistence::Store::open("future") {
        Err(persistence::PersistError::VersionTooNew { found, supported }) => {
            format!("format {found} is newer than {supported}")
        }
        other => format!("unexpected: {other:?}"),
    };
    lines.push(format!("persistence: refused={refused}"));
    lines.push(format!(
        "persistence: why_unknown(player)={} client.level_data={:?}",
        persistence::why_unknown("player"),
        persistence::client::level_data()
    ));

    let _ = std::fs::remove_dir_all(&root);
    if let Some(previous) = previous {
        fs::set_root(previous);
    }
    lines
}
