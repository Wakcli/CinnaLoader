//! Section 71: hot reload and development mode — a real model over the files a mod owns,
//! and an honest refusal for the client's.

use std::sync::{Arc, Mutex};

use cinnaloader::registry;
use cinnaloader::{dev, fs};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    let previous = fs::root();
    let root = std::env::temp_dir().join(format!("cinnaloader-s71-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    fs::set_root(root.clone());

    dev::disable();
    let off = dev::is_enabled();
    dev::enable();
    let on = dev::is_enabled();
    dev::disable();
    lines.push(format!(
        "dev(71): root={} flag off={off} on={on} default_from_env=CINNALOADER_DEV",
        root.display()
    ));

    fs::write_text("assets/lang/en_us.json", "{\"hello\":1}").unwrap();
    let file = fs::resolve("assets/lang/en_us.json").expect("the mod may write its own assets");
    let before = dev::fingerprint("assets/lang/en_us.json").expect("a real file");
    dev::watch_files(&["assets/lang/en_us.json"]);
    let quiet_before = dev::get_changed_files();
    fs::write_text("assets/lang/en_us.json", "{\"hello\":2}").unwrap();
    let after = dev::fingerprint("assets/lang/en_us.json").expect("a real file");
    let changed = dev::get_changed_files();
    let quiet_after = dev::get_changed_files();
    lines.push(format!("dev(71): fingerprint file={}", file.display()));
    lines.push(format!(
        "dev(71): size {}->{} hash {:x}->{:x} changed_only_when_written={}",
        before.size,
        after.size,
        before.hash,
        after.hash,
        before != after
    ));
    lines.push(format!(
        "dev(71): watch quiet_before={quiet_before:?} changed={changed:?} quiet_after={quiet_after:?}"
    ));

    let _ = registry::freeze(None);
    let plan = dev::plan_reload(&[
        "assets/lang/en_us.json".to_owned(),
        "config/settings.toml".to_owned(),
        "scripts/tick.lua".to_owned(),
        "data/recipes.json".to_owned(),
        "shaders/terrain.frag".to_owned(),
        "client/version.json".to_owned(),
    ]);
    for entry in &plan.reloadable {
        lines.push(format!(
            "dev(71): can_reload {} [{}] {}",
            entry.path, entry.kind, entry.reason
        ));
    }
    for entry in &plan.blocked {
        lines.push(format!(
            "dev(71): cannot_reload {} [{}] {}",
            entry.path, entry.kind, entry.reason
        ));
    }
    registry::unfreeze(None);

    let queued = dev::reload_mod().expect("a first request");
    let duplicate = dev::reload_mod();
    let pending = dev::pending_count();
    let running = dev::start_next();
    let running_state = dev::get_reload_status()[0].state;
    let finished = dev::finish(running.expect("the request started"));
    let done = dev::get_reload_status().remove(0);
    lines.push(format!(
        "dev(71): queue id={queued} duplicate_refused={} pending={pending} running={running_state:?} finished={finished} finish={} outcome={:?}",
        duplicate.is_none(),
        done.state,
        done.outcome.as_deref().unwrap_or(""),
    ));

    let order = Arc::new(Mutex::new(Vec::new()));
    for name in ["first", "second"] {
        let into = Arc::clone(&order);
        dev::add_reload_listener(name, dev::ReloadKind::Resources, move || {
            into.lock().unwrap().push(name.to_owned());
        });
    }
    let into = Arc::clone(&order);
    dev::add_reload_listener("config-only", dev::ReloadKind::Config, move || {
        into.lock().unwrap().push("config-only".to_owned());
    });
    fs::write_text("assets/lang/en_us.json", "{\"hello\":3}").unwrap();
    let ran = dev::reload_now();
    let called = order.lock().unwrap().join(",");
    lines.push(format!(
        "dev(71): listeners={:?} ran={ran} in_registration_order=[{called}]",
        dev::get_reload_listeners()
    ));

    for line in dev::what_would_be_lost() {
        lines.push(format!("dev(71): lost_on_reload {line}"));
    }
    lines.push(format!(
        "dev(71): refusals={:?} restart_client={} recompile_shaders={} rebuild_ui={} restart_world={}",
        dev::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
        dev::restart_client(),
        dev::recompile_shaders(),
        dev::rebuild_ui(),
        dev::restart_world(),
    ));
    lines.push(format!("dev(71): why_unknown(client)={}", dev::why_unknown("client")));
    lines.push(format!(
        "dev(71): build={} compile={} log={} notify={} open_mod={:?} open_log={:?} run_command={:?}",
        dev::get_build_info().len(),
        dev::get_compile_info()
            .into_iter()
            .map(|(key, _)| key)
            .collect::<Vec<_>>()
            .join("+"),
        dev::set_log_level("warn"),
        dev::notify("dev demo ran"),
        dev::open_mod_directory().map(|path| path.display().to_string()),
        dev::open_log(),
        dev::run_command("dev", &["status"]),
    ));

    let _ = dev::unwatch_files(&["assets/lang/en_us.json"]);
    let _ = std::fs::remove_dir_all(&root);
    if let Some(previous) = previous {
        fs::set_root(previous);
    }
    lines
}
