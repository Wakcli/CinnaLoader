//! Section 89 demo: the editor document model — a node tree, refusals by name, a cycle, a
//! property path, a two-step undo and redo, a byte-stable round trip and the refusal.

pub fn run() -> Vec<String> {
    use cinnaloader::config::ConfigValue;
    use cinnaloader::editor::{self, NodeKind};

    let mut out = Vec::new();
    editor::clear();

    let mut project = editor::create_project("s89:project", "Demo Editor");
    project.add_node("root", "on_start", NodeKind::Event).expect("a node");
    project.add_node("on_start", "spawn", NodeKind::Action).expect("a node");
    project.add_node("on_start", "give", NodeKind::Action).expect("a node");
    project.add_node("root", "on_tick", NodeKind::Flow).expect("a node");
    let counts = project
        .kind_counts()
        .iter()
        .map(|(kind, count)| format!("{}={}", kind.as_str(), count))
        .collect::<Vec<_>>()
        .join(",");
    out.push(format!(
        "tree(89): nodes={} tree_order={:?} counts={counts}",
        project.node_count(),
        project.tree_order()
    ));

    let duplicate = project.add_node("root", "on_start", NodeKind::Action).unwrap_err();
    let missing = project.add_node("ghost", "orphan", NodeKind::Action).unwrap_err();
    out.push(format!("refuse(89): duplicate='{duplicate}' missing_parent='{missing}'"));

    let cycle = project.move_node("on_start", "spawn").unwrap_err();
    out.push(format!("refuse(89): cycle='{cycle}'"));

    project
        .set_property("on_start.spawn.size", ConfigValue::Float(1.5))
        .expect("a float size");
    let read = project.get_property("on_start.spawn.size").expect("set above");
    let wrong = project
        .set_property("on_start.spawn.size", ConfigValue::String("big".to_owned()))
        .unwrap_err();
    out.push(format!(
        "property(89): list={:?} read={read:?} wrong_kind='{wrong}'",
        project.list_properties("on_start.spawn").expect("a node")
    ));

    project
        .set_property("on_start.spawn.size", ConfigValue::Float(2.5))
        .expect("a float");
    project
        .set_property("on_start.spawn.size", ConfigValue::Float(3.5))
        .expect("a float");
    assert!(project.undo() && project.undo(), "two steps back");
    let after_undo = project.get_property("on_start.spawn.size").expect("set above");
    assert!(project.redo() && project.redo(), "two steps forward");
    let after_redo = project.get_property("on_start.spawn.size").expect("set above");
    out.push(format!(
        "undo(89): after_two_undo={after_undo:?} after_two_redo={after_redo:?} undo={} redo={}",
        project.undo_depth(),
        project.redo_depth()
    ));

    let text = editor::save_project(&project);
    let loaded = editor::import_project(&text).expect("well formed");
    let again = editor::save_project(&loaded);
    assert_eq!(again, text, "a byte-stable round trip");
    out.push(format!(
        "save(89): bytes={} stable={} loaded_nodes={} loaded_id={}",
        text.len(),
        again == text,
        loaded.node_count(),
        loaded.id
    ));

    let preview = project.preview();
    out.push(format!(
        "preview(89): summary={:?} lines={}",
        preview.lines().next().unwrap_or_default(),
        preview.lines().count()
    ));

    let refused = editor::open_project("s89:project");
    assert!(refused.is_err());
    out.push(format!("refuse(89): why={}", editor::why_unknown("editor_window")));
    out.push(format!(
        "refuse(89): entrypoint={} listed={:?}",
        refused.is_err(),
        editor::refusals()
            .iter()
            .map(|(what, _)| what.clone())
            .collect::<Vec<_>>()
    ));

    out
}
