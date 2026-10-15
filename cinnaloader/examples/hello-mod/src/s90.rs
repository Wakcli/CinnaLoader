//! Section 90 demo: the docs registry and the API surface — a ranked search, a rendered
//! Markdown fragment, a refused duplicate, a lookup by path, an exact version diff and the
//! refusal sentence.

pub fn run() -> Vec<String> {
    use cinnaloader::api::{self, ChangeKind};
    use cinnaloader::docs::{self, DocKind};

    let mut out = Vec::new();
    docs::clear();
    api::clear();

    docs::register(
        docs::Entry::new("blocks::get", "look up a block")
            .kind(DocKind::ApiReference)
            .param("id", "string", true)
            .returns("Block")
            .example("let block = get(\"minecraft:stone\");")
            .since("0.2.0")
            .tag("world"),
    );
    docs::register_page("blocks::getter", "the getter form");
    docs::register_page("items::lookup", "mirrors blocks::get without a block");

    let hits = docs::search("blocks::get", 8);
    let ranked: Vec<String> = hits
        .iter()
        .map(|hit| format!("[{}]{} order={}", hit.rank.as_str(), hit.entry.name, hit.rank.order()))
        .collect();
    out.push(format!(
        "docs(90): search blocks::get -> {} (exact name first, name prefix next, summary hit last)",
        ranked.join(", ")
    ));

    let markdown = docs::generate_reference_for("blocks::get").expect("documented");
    let first = markdown.lines().next().unwrap_or("");
    let third = markdown.lines().nth(2).unwrap_or("");
    out.push(format!(
        "docs(90): markdown line1='{first}' line3='{third}' bytes={}",
        markdown.len()
    ));

    let refused = !docs::register_page("blocks::getter", "again");
    out.push(format!(
        "docs(90): duplicate refused={refused} entries={}",
        docs::get_api_index().len()
    ));

    api::clear();
    api::register_module("cinnaloader::blocks");
    api::register_function("cinnaloader::blocks::get", "fn get(id: &str) -> Option<Block>");
    let item = api::get("cinnaloader::blocks::get").expect("declared");
    out.push(format!(
        "api(90): lookup cinnaloader::blocks::get kind={} stability={} signature={}",
        item.kind.as_str(),
        item.stability.as_str(),
        item.signature
    ));

    api::record_change(ChangeKind::Added, "0.2.0", "cinnaloader::blocks::get", "");
    api::record_change(ChangeKind::Removed, "0.2.0", "cinnaloader::blocks::old_get", "");
    api::record_change(
        ChangeKind::Changed,
        "0.2.0",
        "cinnaloader::blocks::get_by_id",
        "now takes a BlockId",
    );
    api::record_change(
        ChangeKind::Deprecated,
        "0.2.0",
        "cinnaloader::blocks::legacy",
        "use cinnaloader::blocks::get",
    );
    let window = api::diff("0.1.0", "0.2.0");
    out.push(format!(
        "api(90): diff {}..{} +{:?} -{:?} ~{:?} !{:?}",
        window.from, window.to, window.added, window.removed, window.changed, window.deprecated
    ));

    let why = docs::why_unknown("method");
    out.push(format!("docs(90): refusal {} -> {why}", docs::refusals().len()));

    out
}
