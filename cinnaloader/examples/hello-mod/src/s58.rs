//! Section 58 demo.

use cinnaloader::packs::{self, PackKind, PackManifest};

pub fn run() -> Vec<String> {
    let mut out = Vec::new();
    let alpha = PackManifest::new("s58-alpha", "Alpha")
        .description("low priority pack")
        .format(packs::SUPPORTED_FORMAT)
        .kind(PackKind::Both)
        .priority(0);
    let over = PackManifest::new("s58-over", "Overlay")
        .description("high priority data overlay")
        .kind(PackKind::Data)
        .priority(9);
    out.push(format!(
        "packs(58): create s58-alpha={} s58-over={}",
        packs::create(alpha),
        packs::create(over)
    ));
    let base_text = "smooth=alpha\nunique=alpha\n";
    let over_text = "smooth=over\n";
    packs::insert_text("s58-alpha", "data/tags.txt", base_text);
    packs::insert_text("s58-over", "data/tags.txt", over_text);
    out.push(format!(
        "packs(58): s58-alpha:data/tags.txt={} bytes s58-over:data/tags.txt={} bytes",
        base_text.len(),
        over_text.len()
    ));
    let (winner, bytes) = packs::resolve("data/tags.txt").expect("a winner");
    out.push(format!(
        "packs(58): resolve data/tags.txt -> winner={winner} bytes={} providers={:?}",
        bytes.len(),
        packs::providers("data/tags.txt")
    ));
    let merged = packs::merge("data/tags.txt").expect("a merged file");
    out.push(format!(
        "packs(58): merge data/tags.txt -> {} keys smooth={:?} unique={:?}",
        merged.entries.len(),
        merged.entries.get("smooth"),
        merged.entries.get("unique")
    ));
    packs::disable("s58-over");
    let (winner, bytes) = packs::resolve("data/tags.txt").expect("the promoted winner");
    out.push(format!(
        "packs(58): disable(s58-over) -> resolve data/tags.txt -> winner={winner} bytes={}",
        bytes.len()
    ));
    let future = PackManifest::new("s58-future", "Future").format(packs::SUPPORTED_FORMAT + 1);
    out.push(format!(
        "packs(58): format gate create(s58-future format={}) refused={} errors={:?}",
        packs::SUPPORTED_FORMAT + 1,
        packs::create(future),
        packs::get_activation_errors()
    ));
    out.push(format!(
        "packs(58): active_stack={:?} enabled={} disabled={} client_list={:?}",
        packs::get_active_stack(),
        packs::get_enabled().len(),
        packs::get_disabled().len(),
        packs::known_only_to_the_client::get_mounted()
    ));
    packs::uninstall("s58-alpha");
    packs::uninstall("s58-over");
    out
}
