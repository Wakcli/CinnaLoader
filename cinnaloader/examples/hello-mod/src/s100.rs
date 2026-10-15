//! Section 100 demo: a build's description — the plan's exact command line, an ABI answer
//! naming both versions, an artifact list with its total, a log's slowest step, an
//! incremental decision naming which steps rerun and which are reused, and the refusal.

pub fn run() -> Vec<String> {
    use cinnaloader::build::{self, Artifact, BuildConfig, Outcome, Profile, StepRun, Target};

    let mut out = Vec::new();

    let target = Target::new("x86_64-unknown-linux-gnu").feature("fast");
    let config = BuildConfig::new(Profile::release(), target.clone()).cache("out/cache");
    let plan = build::plan(&config);
    let first = plan.first().expect("a build invocation");
    out.push(format!(
        "build(100): plan program={} args={:?} cwd={}",
        first.program, first.args, first.cwd
    ));
    out.push(format!(
        "build(100): rustflags={:?} abi_env={}",
        first.env[1].1, first.env[2].1
    ));

    out.push(format!(
        "build(100): target {} -> {}",
        target.triple,
        target.compatibility().sentence()
    ));
    let other = Target::new("aarch64-apple-darwin").abi(cinnaloader::ABI_VERSION + 1);
    out.push(format!(
        "build(100): other {} -> {} compatible={}",
        other.triple,
        other.compatibility().sentence(),
        other.is_compatible()
    ));

    let files = vec![
        ("build/mod.so".to_owned(), vec![1, 2, 3, 4]),
        ("build/assets.bin".to_owned(), vec![5, 6]),
    ];
    let artifacts: Vec<Artifact> = build::package(&files);
    out.push(format!(
        "build(100): artifacts={} total={} names={:?}",
        artifacts.len(),
        build::total_size(&artifacts),
        artifacts.iter().map(|artifact| artifact.name.clone()).collect::<Vec<_>>()
    ));

    let log = build::build_log(vec![
        StepRun {
            name: "compile".to_owned(),
            duration_ms: 120,
            outcome: Outcome::Ok,
            message: "built".to_owned(),
        },
        StepRun {
            name: "link".to_owned(),
            duration_ms: 400,
            outcome: Outcome::Failed,
            message: "no linker".to_owned(),
        },
        StepRun {
            name: "sign".to_owned(),
            duration_ms: 30,
            outcome: Outcome::Skipped,
            message: "not reached".to_owned(),
        },
    ]);
    let slowest = log.slowest().expect("a step");
    out.push(format!(
        "build(100): log total={}ms slowest={} {}ms failed={}",
        log.total_ms(),
        slowest.name,
        slowest.duration_ms,
        log.failed().len()
    ));
    build::record_log(log);

    build::register_build_step("compile", &["src/lib.rs"]);
    build::register_build_step("assets", &["assets/icon.png"]);
    build::register_build_step("package", &[]);
    build::update_inputs(&[
        ("src/lib.rs".to_owned(), 11),
        ("assets/icon.png".to_owned(), 22),
    ]);
    let decision = build::update_inputs(&[
        ("src/lib.rs".to_owned(), 11),
        ("assets/icon.png".to_owned(), 99),
    ]);
    out.push(format!(
        "build(100): incremental rerun={:?} reused={:?}",
        decision.rerun, decision.reused
    ));

    out.push(format!(
        "build(100): report bytes={} hash={} json={}",
        build::report().len(),
        build::get_build_hash(),
        build::report_json().starts_with('{')
    ));

    let refused = build::compile().expect_err("this host has no compiler");
    out.push(format!("build(100): refused why={refused}"));
    out.push(format!(
        "build(100): refusals={:?}",
        build::refusals()
            .iter()
            .map(|(what, _)| what.clone())
            .collect::<Vec<_>>()
    ));

    out
}
