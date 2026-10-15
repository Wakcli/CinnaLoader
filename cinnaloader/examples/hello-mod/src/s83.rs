//! Section 83 demo.

use cinnaloader::{fs, installer};

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();
    let previous = fs::root();
    let root = std::env::temp_dir().join(format!("cinnaloader-s83-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    fs::set_root(root.clone());

    let _ = fs::write_text("pkg/core.txt", "core body");
    let _ = fs::write_text("pkg/replaced.txt", "package version");
    let _ = fs::write_text("pkg/manifest.txt", "id=alpha\nname=Alpha\nversion=2.0.0\n");
    let _ = fs::write_text("game/replaced.txt", "pre-existing version");
    let _ = fs::write_text("game/preexisting.txt", "left in place");

    let plan = installer::plan("pkg", "game").expect("a plan over both trees");
    lines.push(format!(
        "plan: target={} added={:?} replaced={:?} kept={:?}",
        fs::resolve("game").expect("the target").display(),
        plan.added(),
        plan.replaced(),
        plan.kept()
    ));
    for entry in &plan.entries {
        lines.push(format!("plan: {} is {} — {}", entry.path, entry.action, entry.reason));
    }

    let journal = installer::install("pkg", "game", "alpha").expect("a real install");
    lines.push(format!(
        "install: name={} version={} steps={} target={}",
        journal.name,
        journal.version,
        journal.steps.len(),
        fs::resolve(&journal.target).expect("the target").display()
    ));
    for step in journal.completed() {
        lines.push(format!("install: step {} {} — {}", step.path, step.action, step.reason));
    }

    let clean = installer::verify_checksum("alpha").expect("an installed package");
    let _ = fs::write_text("game/core.txt", "tampered after install");
    let dirty = installer::verify_checksum("alpha").expect("an installed package");
    lines.push(format!(
        "verify: clean={clean:?} after_tamper={dirty:?} at {}",
        fs::resolve("game/core.txt").expect("a file").display()
    ));

    let removed = installer::uninstall("alpha");
    lines.push(format!(
        "uninstall: removed={removed} added_gone={} preexisting_survives={} replaced_restored={} at {}",
        !fs::exists("game/core.txt"),
        fs::exists("game/preexisting.txt"),
        fs::read_text("game/replaced.txt").unwrap_or_default() == "pre-existing version",
        fs::resolve("game/preexisting.txt").expect("a file").display()
    ));

    let _ = fs::write_text("game/rollback.txt", "before install");
    let _ = fs::write_text("pkg2/rollback.txt", "after install");
    let _ = fs::write_text("pkg2/manifest.txt", "id=beta\nversion=1.0.0\n");
    let before = fs::read_text("game/rollback.txt").unwrap_or_default();
    installer::install("pkg2", "game", "beta").expect("a second install");
    let changed = fs::read_text("game/rollback.txt").unwrap_or_default();
    let rolled = installer::rollback("beta");
    let restored = fs::read_text("game/rollback.txt").unwrap_or_default();
    lines.push(format!(
        "rollback: rolled={rolled} changed_to={changed:?} restored_exactly={} at {}",
        restored == before,
        fs::resolve("game/rollback.txt").expect("a file").display()
    ));

    let _ = fs::write_text("v2/g.txt", "graph version two");
    let _ = fs::write_text("v2/manifest.txt", "id=gamma\nversion=2.0.0\n");
    installer::install("v2", "game", "gamma").expect("an install to update");
    let _ = fs::write_text("v1/g.txt", "graph version one");
    let _ = fs::write_text("v1/manifest.txt", "id=gamma\nversion=1.0.0\n");
    let refused = match installer::update("v1", "gamma", false) {
        Err(error) => error.to_string(),
        Ok(_) => "unexpectedly accepted".to_owned(),
    };
    let allowed = installer::update("v1", "gamma", true).is_ok();
    lines.push(format!(
        "update: refused_downgrade={refused} | allowed_when_asked={allowed} now={}",
        fs::read_text("game/g.txt").unwrap_or_default()
    ));

    for (what, why) in installer::refusals() {
        lines.push(format!("refusal: {what} — {why}"));
    }

    let _ = std::fs::remove_dir_all(&root);
    if let Some(previous) = previous {
        fs::set_root(previous);
    }
    lines
}
