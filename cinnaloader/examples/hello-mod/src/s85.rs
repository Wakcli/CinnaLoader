//! Section 85 demo: the loader's config document, its service registry, its state machine, a
//! phase report with exact durations, and the two things a mod may not write.

pub fn run() -> Vec<String> {
    use cinnaloader::fs;
    use cinnaloader::lifecycle::Phase;
    use cinnaloader::loader::{
        self, LoaderConfig, LoaderState, LogLevel, PhaseReport, ServiceRequirement, LOGGER,
        MOD_MANAGER,
    };
    use std::path::PathBuf;
    use std::time::Duration;

    let mut out = Vec::new();
    let previous = fs::root();
    let root = std::env::temp_dir().join(format!("cinnaloader-s85-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    fs::set_root(root.clone());

    let config = LoaderConfig {
        mod_directories: vec![PathBuf::from("mods"), PathBuf::from("extra")],
        disabled_mods: vec!["s85:off".to_owned()],
        load_order: vec!["core".to_owned(), "extra".to_owned()],
        log_level: LogLevel::Debug,
        dev: true,
    };
    let wrote = config.write().is_ok();
    let read = LoaderConfig::read().expect("the loader config reads back");
    let equal = read == config;
    assert!(wrote && equal, "the config round trips through the file");
    out.push(format!(
        "loader(85): config round trip wrote={wrote} equal={equal} dirs={:?} log={} dev={}",
        read.mod_directories, read.log_level, read.dev
    ));

    let unknown = LoaderConfig::defaults().set("mystery", "1");
    assert_eq!(
        unknown,
        Err(loader::ConfigError::UnknownKey("mystery".to_owned()))
    );
    out.push(format!(
        "loader(85): unknown key refused by name = {:?}",
        unknown.unwrap_err()
    ));

    let wrong = LoaderConfig::defaults().set("log_level", "true");
    assert!(matches!(wrong, Err(loader::ConfigError::WrongType { .. })));
    out.push(format!(
        "loader(85): wrong type refused with both kinds = {}",
        wrong.unwrap_err()
    ));

    let manager = loader::get_service(MOD_MANAGER).expect("the loader publishes its mod manager");
    let fits = manager.supports(&ServiceRequirement::new("mods", "1.0.0"));
    assert!(fits.is_ok());
    let logger = loader::get_service(LOGGER).expect("and a logger");
    let lacks = logger.supports(&ServiceRequirement::new("permissions", "1.0.0"));
    assert!(matches!(
        lacks,
        Err(loader::ServiceError::MissingCapability { .. })
    ));
    out.push(format!(
        "loader(85): {} resolves mods={} {} lacks ({})",
        manager.name,
        fits.is_ok(),
        logger.name,
        lacks.unwrap_err()
    ));

    let illegal = LoaderState::Starting.transition(LoaderState::Ready);
    assert_eq!(
        illegal,
        Err(loader::TransitionError {
            from: LoaderState::Starting,
            to: LoaderState::Ready,
        })
    );
    out.push(format!(
        "loader(85): illegal state transition refused = {}",
        illegal.unwrap_err()
    ));

    let mut report = PhaseReport::new();
    report.record(Phase::Load, "reading manifests", Duration::from_millis(5));
    report.record(Phase::Initialize, "wiring mods", Duration::from_millis(3));
    assert_eq!(report.total(), Duration::from_millis(8));
    out.push(format!(
        "loader(85): report phases={} total={:?} load={:?} slowest={}",
        report.len(),
        report.total(),
        report.phase_time(Phase::Load),
        report
            .slowest()
            .map_or("none".to_owned(), |t| t.what.clone())
    ));

    let wrote_config = loader::set_config("log_level", "trace");
    let wrote_state = loader::set_state(LoaderState::Ready);
    assert!(!wrote_config && !wrote_state);
    out.push(format!(
        "loader(85): a mod may read but not write config={wrote_config} state={wrote_state} why={}",
        loader::why_unknown("config")
    ));

    let _ = std::fs::remove_dir_all(&root);
    if let Some(previous) = previous {
        fs::set_root(previous);
    }
    out
}
