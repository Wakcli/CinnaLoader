//! Section 88 demo: the introspection surface a tool reads, with counts, a lookup and its
//! refusal, a stable export, an exact diff, a validation pass and the refusal sentence.

pub fn run() -> Vec<String> {
    use cinnaloader::content::ContentKind;
    use cinnaloader::tools::{self, Change, Kind, ToolKind};

    let mut out = Vec::new();
    tools::clear();
    tools::clear_selection();

    tools::register_command("s88:hello");
    tools::register_inspector("s88:inspect");
    tools::register_exporter("s88:export");
    tools::register_needing(ToolKind::Validator, "s88:rule", &["s88:missing"]);

    let command = tools::get(ToolKind::Command, "s88:hello").expect("declared");
    out.push(format!(
        "tools(88): tools={} command={} inspector={} exporter={} validator={} first={}/{} owner={}",
        tools::get_all().len(),
        tools::tools_of(ToolKind::Command).len(),
        tools::tools_of(ToolKind::Inspector).len(),
        tools::tools_of(ToolKind::Exporter).len(),
        tools::tools_of(ToolKind::Validator).len(),
        command.kind.as_str(),
        command.id,
        command.owner
    ));

    let counts = tools::counts();
    out.push(format!(
        "inventory(88): items={} kinds={} command={} inspector={} exporter={} validator={}",
        tools::inventory().len(),
        counts.len(),
        tools::inventory_of_kind(Kind::Tool(ToolKind::Command)).len(),
        tools::inventory_of_kind(Kind::Tool(ToolKind::Inspector)).len(),
        tools::inventory_of_kind(Kind::Tool(ToolKind::Exporter)).len(),
        tools::inventory_of_kind(Kind::Tool(ToolKind::Validator)).len()
    ));

    let found = tools::lookup(Kind::Tool(ToolKind::Command), "s88:hello");
    let missing = tools::lookup(Kind::Tool(ToolKind::Command), "s88:nope");
    out.push(format!(
        "lookup(88): found={:?} missing={:?}",
        found.map(|item| item.id),
        missing.unwrap_err().to_string()
    ));

    let first = tools::export();
    let second = tools::export();
    assert_eq!(first, second, "two exports compare byte for byte");
    out.push(format!(
        "export(88): bytes={} stable={} head={}",
        first.len(),
        first == second,
        first.chars().take(56).collect::<String>()
    ));

    tools::clear();
    tools::register_command("s88:alpha");
    tools::register_command("s88:beta");
    let before = tools::export();
    tools::register_command("s88:gamma");
    tools::unregister(ToolKind::Command, "s88:beta");
    tools::unregister(ToolKind::Command, "s88:alpha");
    tools::register_needing(ToolKind::Command, "s88:alpha", &["s88:gamma"]);
    let after = tools::export();
    let changed = tools::diff(&before, &after).expect("both parse");
    let label = |changes: &[Change]| changes.iter().map(|entry| entry.to_string()).collect::<Vec<_>>();
    assert_eq!(label(&changed.added), ["command s88:gamma"]);
    assert_eq!(label(&changed.removed), ["command s88:beta"]);
    assert_eq!(label(&changed.changed), ["command s88:alpha"]);
    out.push(format!(
        "diff(88): added={:?} removed={:?} changed={:?}",
        label(&changed.added),
        label(&changed.removed),
        label(&changed.changed)
    ));

    tools::clear();
    tools::register_needing(ToolKind::Validator, "s88:rule", &["s88:missing"]);
    let live = tools::validate();
    let ours = live
        .problems
        .iter()
        .find(|problem| problem.id == "s88:rule")
        .expect("reported");
    assert_eq!(ours.problem, "needs 's88:missing' which is not declared");
    out.push(format!(
        "validate(88): live={} entry={} {}: {}",
        live.count(),
        ours.kind,
        ours.id,
        ours.problem
    ));

    let wire = r#"{"format":1,"items":[
        {"source":"content","kind":"block","id":"s88:dup","owner":"hello","needs":[]},
        {"source":"content","kind":"block","id":"s88:dup","owner":"hello","needs":[]},
        {"source":"content","kind":"item","id":"s88:ref","owner":"hello","needs":["s88:missing"]}]}"#;
    let document = tools::import(wire).expect("well formed");
    let report = tools::validate_document(&document);
    assert_eq!(report.count(), 2);
    let labelled: Vec<String> = report
        .problems
        .iter()
        .map(|problem| format!("{} {}", problem.kind, problem.id))
        .collect();
    out.push(format!(
        "validate(88): imported problems={} entries={:?}",
        report.count(),
        labelled
    ));

    let refused = tools::get_editor_context();
    assert!(refused.is_err());
    out.push(format!("refuse(88): why={}", tools::why_unknown("editor_context")));
    out.push(format!(
        "refuse(88): entrypoint={} listed={:?}",
        refused.is_err(),
        tools::refusals()
            .iter()
            .map(|(what, _)| what.clone())
            .collect::<Vec<_>>()
    ));

    let schema = tools::schema(Kind::Tool(ToolKind::Command)).expect("a known kind");
    let content_schema = tools::schema(Kind::Content(ContentKind::Block)).expect("a known kind");
    assert!(schema.contains("\"name\":\"needs\""));
    assert!(content_schema.contains("\"name\":\"client_registry\""));
    out.push(format!(
        "schema(88): tool fields={} content fields={} has_needs={} has_client_registry={}",
        schema.matches("\"name\":").count(),
        content_schema.matches("\"name\":").count(),
        schema.contains("\"name\":\"needs\""),
        content_schema.contains("\"name\":\"client_registry\"")
    ));

    out
}
