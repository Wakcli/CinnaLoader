//! Section 59 demo.

pub fn run() -> Vec<String> {
    use cinnaloader::content::{self, ContentKind};
    let mut out = Vec::new();

    out.push(format!(
        "content(59): kinds={} register block={} item={} recipe={} fluid={} statistic={}",
        ContentKind::all().len(),
        content::register_block("s59:lantern"),
        content::register_item("s59:gem"),
        content::register_recipe("s59:craft"),
        content::register_fluid("s59:slime"),
        content::register_statistic("s59:mined"),
    ));
    out.push(format!(
        "content(59): duplicate block={} duplicate item={} (second call per id is false)",
        content::register_block("s59:lantern"),
        content::register_item("s59:gem"),
    ));
    out.push(format!(
        "content(59): read back through owners block={:?} item={:?} recipe={:?} fluid={:?}",
        cinnaloader::blocks::get_type("s59:lantern").map(|block| block.id),
        cinnaloader::items::get("s59:gem").map(|item| item.id),
        cinnaloader::recipes::get_by_id("s59:craft").map(|recipe| recipe.id),
        cinnaloader::fluids::get_fluid("s59:slime").map(|fluid| fluid.id),
    ));
    out.push(format!(
        "content(59): counts block={} item={} recipe={} fluid={} statistic={} kinds_present={}",
        content::count(ContentKind::Block),
        content::count(ContentKind::Item),
        content::count(ContentKind::Recipe),
        content::count(ContentKind::Fluid),
        content::count(ContentKind::Statistic),
        content::kinds().len(),
    ));
    let entry = content::get(ContentKind::Block, "s59:lantern");
    out.push(format!(
        "content(59): entry={:?} owner={:?} module={:?}",
        entry.as_ref().map(|entry| entry.id.clone()),
        entry.as_ref().map(|entry| entry.owner.clone()),
        entry.as_ref().map(|entry| entry.module()),
    ));
    out.push(format!(
        "content(59): refuse item={} sentence={}",
        content::register_with_the_client(ContentKind::Item, "s59:gem"),
        content::why_unknown("item"),
    ));
    out.push(format!(
        "content(59): refusal sentences={} sample_block={:?}",
        content::refusals().len(),
        content::why_unknown("block"),
    ));
    out.push(format!(
        "content(59): unregister block={} count={} all={}",
        content::unregister(ContentKind::Block, "s59:lantern"),
        content::count(ContentKind::Block),
        content::all().len(),
    ));
    out
}
