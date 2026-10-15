//! Section 104 demo: the utilities that need no client, each at its exact boundary — a
//! UTF-8-safe truncation, the byte and duration formatters, a rejected id, a parsed colour
//! and duration, a case conversion, a tie-broken top-N, and a dedupe that kept its order.

pub fn run() -> Vec<String> {
    use cinnaloader::utils;

    let mut out = Vec::new();

    let title = "héllo wörld";
    let cut = utils::truncate_text(title, 6);
    out.push(format!(
        "utils(104): truncate {title:?} at 6 -> {cut:?} exact={} chars={} bytes={} intact={}",
        cut == "héllo…",
        cut.chars().count(),
        cut.len(),
        title.starts_with("héllo"),
    ));

    out.push(format!(
        "utils(104): bytes 999={:?} 1000={:?} 1024={:?} 1000000={:?} 1048576={:?}",
        utils::humanize_bytes(999),
        utils::humanize_bytes(1000),
        utils::humanize_bytes(1024),
        utils::humanize_bytes(1_000_000),
        utils::humanize_bytes(1_048_576),
    ));

    out.push(format!(
        "utils(104): duration compact 0={:?} 1000={:?} 90000={:?} 9000000={:?} long 9000000={:?}",
        utils::format_duration(0),
        utils::format_duration(1000),
        utils::format_duration(90_000),
        utils::format_duration(9_000_000),
        utils::format_duration_long(9_000_000),
    ));

    let bad = utils::parse_resource_id("mod:../escape");
    let good = utils::parse_resource_id("mymod:textures/x.png").expect("a valid id");
    out.push(format!(
        "utils(104): id refused by rule err={:?} good={}",
        bad.map(|_| ()).map_err(|error| error.to_string()),
        utils::format_resource_id(&good),
    ));

    let color = utils::parse_color("#ff8800").expect("a hex colour");
    out.push(format!(
        "utils(104): color #ff8800 -> rgba={:?}",
        color.to_array(),
    ));

    out.push(format!(
        "utils(104): duration \"2h30m\" -> {}ms; \"2h30x\" -> {:?}",
        utils::parse_duration("2h30m").expect("a duration"),
        utils::parse_duration("2h30x").map(|_| ()).map_err(|error| error.to_string()),
    ));

    out.push(format!(
        "utils(104): case HTTPServer -> snake={:?} camel={:?}",
        utils::to_snake_case("HTTPServer"),
        utils::to_camel_case("HTTPServer"),
    ));

    let scores = [("a", 5), ("b", 5), ("c", 3), ("d", 7)];
    let top = utils::top_n_by(&scores, 2, |item| item.1);
    out.push(format!(
        "utils(104): top_n 2 by score -> {top:?} (tie a before b, first-seen)",
    ));

    let ordered = ["b", "a", "b", "c", "a"];
    out.push(format!(
        "utils(104): dedupe {ordered:?} -> {:?}",
        utils::deduplicate(&ordered),
    ));

    out
}
