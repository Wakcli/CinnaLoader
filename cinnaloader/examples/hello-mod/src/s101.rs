//! Section 101 demo: a generator that really writes. A template rendered with a missing value
//! refused by name, a batch with its real byte total, an incremental skip and a rewrite with
//! both hashes, a manifest's total and a verify naming a changed file, a dry run that leaves
//! the file alone, and the refusal.

use cinnaloader::fs;
use cinnaloader::generators::{self, Generator, OutputKind, OutputFormat, Template};

pub fn run() -> Vec<String> {
    if fs::root().is_none() {
        let root = std::env::temp_dir().join(format!("cinnaloader-demo-101-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        fs::set_root(root);
    }
    let mut out = Vec::new();

    let _ = generators::register_template(Template::text("page", "title={{title}} value={{value}}\n"));
    let _ = generators::register_template(Template::text("entry", "{\"title\": \"{{title}}\"}"));

    match generators::render_template("entry", &[("other".to_owned(), "x".to_owned())]) {
        Ok(text) => out.push(format!("generators(101): template unexpectedly rendered {text}")),
        Err(error) => out.push(format!("generators(101): template refused missing value -> {error}")),
    }

    let _ = generators::register(
        Generator::new("pages", OutputKind::Documentation)
            .root("demo101/pages")
            .target("{{name}}.txt")
            .format(OutputFormat::Text)
            .template("page"),
    );

    let inputs: Vec<Vec<(String, String)>> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|name| {
            vec![
                ("name".to_owned(), (*name).to_owned()),
                ("title".to_owned(), format!("{name} title")),
                ("value".to_owned(), name.len().to_string()),
            ]
        })
        .collect();
    let batch = generators::run_all("pages", &inputs).expect("a batch");
    out.push(format!(
        "generators(101): batch written={} bytes={} files={:?}",
        batch.written,
        batch.bytes,
        batch.outputs.iter().map(|output| output.path.clone()).collect::<Vec<_>>()
    ));

    let same = generators::run_all("pages", &inputs[..1]).expect("a rerun");
    let first_hash = same.outputs[0].hash;
    let changed = generators::run_all(
        "pages",
        &[vec![
            ("name".to_owned(), "alpha".to_owned()),
            ("title".to_owned(), "alpha title changed".to_owned()),
            ("value".to_owned(), "alpha".len().to_string()),
        ]],
    )
    .expect("a rewrite");
    out.push(format!(
        "generators(101): incremental skipped={} hash={:#018x} rewritten={} hash={:#018x}",
        same.outputs[0].skipped, first_hash, !changed.outputs[0].skipped, changed.outputs[0].hash
    ));

    let manifest = generators::generate_manifest("pages").expect("a manifest");
    let victim = manifest.entries[0].path.clone();
    fs::write_text(&victim, "changed after the manifest\n").expect("a change");
    out.push(format!(
        "generators(101): manifest total={} entries={} verify={:?}",
        manifest.total,
        manifest.entries.len(),
        manifest.verify()
    ));

    let _ = generators::register(
        Generator::new("draft", OutputKind::Sources)
            .root("demo101/draft")
            .target("{{name}}.txt")
            .template("page"),
    );
    let planned = generators::dry_run(
        "draft",
        &[vec![
            ("name".to_owned(), "delta".to_owned()),
            ("title".to_owned(), "d".to_owned()),
            ("value".to_owned(), "9".to_owned()),
        ]],
    )
    .expect("a dry run");
    out.push(format!(
        "generators(101): dry_run would_write={} bytes={} exists_after={}",
        planned.written,
        planned.bytes,
        fs::exists("demo101/draft/delta.txt")
    ));

    let _ = generators::register(
        Generator::new("client_assets", OutputKind::Assets)
            .root("client/assets")
            .target("{{name}}.txt")
            .template("page"),
    );
    let refused = generators::run(
        "client_assets",
        &[
            ("name".to_owned(), "x".to_owned()),
            ("title".to_owned(), "x".to_owned()),
            ("value".to_owned(), "1".to_owned()),
        ],
    )
    .expect_err("client assets are refused");
    out.push(format!("generators(101): refused -> {refused}"));
    out.push(format!(
        "generators(101): refusals={:?}",
        generators::refusals().iter().map(|(what, _)| what.clone()).collect::<Vec<_>>()
    ));

    let _ = generators::unregister("pages");
    let _ = generators::unregister("draft");
    let _ = generators::unregister("client_assets");
    let _ = generators::unregister_template("page");
    let _ = generators::unregister_template("entry");
    fs::delete("demo101").ok();

    out
}
