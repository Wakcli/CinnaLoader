//! Example Cinnaloader mod: one of every setting kind, a keybind and a timer.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cinnaloader::prelude::*;

/// Whether a section's demonstration should run: every one of them when nothing is set, or
/// the one a run asked for, so a verification run is small and its log is short.
fn demo(section: u32) -> bool {
    match std::env::var("CINNALOADER_DEMO") {
        Ok(wanted) => wanted == "all" || wanted == section.to_string(),
        Err(_) => true,
    }
}

/// Ticks seen, for a log line that would otherwise repeat every frame.
static TICKS: AtomicU32 = AtomicU32::new(0);

/// How many times the repeating task has run, so the log shows it stopping.
static REPEATS: AtomicU32 = AtomicU32::new(0);

fn self_ticks() -> u32 {
    TICKS.load(Ordering::Relaxed)
}

#[derive(Default)]
struct Hello {
    ticks: AtomicU32,
    /// The repeating task, for cancelling later.
    beat_task: TaskHandle,
    /// A worker's result, read on the tick.
    worker: Option<TaskResult<(u64, String, bool)>>,
    pool_result: Option<TaskResult<&'static str>>,
    /// Fires about once a second, wherever the loader ticks it.
    beat: Timer,
    /// Frames seen, so the render hooks can prove themselves once instead of every frame.
    frames: AtomicU32,
}

impl Mod for Hello {
    fn load(&mut self) {
        declare(
            ModManifest::new("hello_mod", "Hello mod", env!("CARGO_PKG_VERSION"))
                .contributors(&["you"])
                .credit("the Cinnabar agent docs")
                .license("MIT")
                .website("https://github.com/Wakcli/CinnaLoader")
                .source("https://github.com/Wakcli/CinnaLoader")
                .tag("settings")
                .category("Display")
                .environment(Environment::Client)
                .capability(Capability::PlayerRead)
                // The rest of the manifest was declared at build time, next to the
                // dependency on core_lib; this call amends it.
                .requires_game(">=1.26, <1.27")
                .supports_game("1.26.0", "1.26.99")
                .requires_platform(std::env::consts::OS)
                .optional_feature("world_read")
                .build(BuildInfo {
                    number: option_env!("BUILD_NUMBER").and_then(|number| number.parse().ok()),
                    git_commit: option_env!("GIT_COMMIT").map(str::to_owned),
                    timestamp: None,
                    development: cfg!(debug_assertions),
                }),
        );
        info!(
            "manifest: declared={} dependencies={:?} tags={:?}",
            declared().is_some(),
            manifest().dependencies.iter().map(|d| d.id.clone()).collect::<Vec<_>>(),
            manifest().tags,
        );
        // Asking the loader about itself, which is section 1 of the wishlist.
        info!(
            "identity: {} v{} [{}] by {} | state={} enabled={} dev={} | dir={} | ns={}",
            name(),
            version(),
            license().unwrap_or_else(|| "no licence".to_owned()),
            authors().join(", "),
            state(),
            is_enabled(),
            is_development(),
            directory()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unknown".to_owned()),
            namespace().scope("watermark"),
        );
        info!(
            "manifest v{} | tags {:?} | declared capabilities {} | loader v{} | abi {}",
            manifest_version(),
            tags(),
            declared_capabilities().len(),
            loader_version(),
            compat::api(),
        );
        info!(
            "compat: game {} ({}, {}, protocol {:?}) | supported: {} | optional {:?}",
            compat::game(),
            compat::game_edition(),
            compat::game_platform(),
            compat::game_protocol(),
            compat::is_game_version_supported(&VersionReq::parse(">=1.26, <1.27").unwrap()),
            compat::feature_flags(),
        );
        feature("watermark", "Watermark", "Display", ON);
        slider("scale", "Watermark scale", "Display", 1.0, 0.5..=3.0, 0.1);
        choice(
            "corner",
            "Watermark corner",
            "Display",
            &["Top left", "Top right", "Bottom left", "Bottom right"],
            1,
        );
        keybind("panic", "Panic (turn everything off)", "Debug", "P");
        button("reset", "Reset settings", "Debug");

        // Section 6: the events the loader fires for this mod, and the ones it queues.
        info!("log: before load's own lines, this is the first thing written");
        events::register_event_type("hello:ready");
        events::subscribe(kind::GAME_STARTED, Priority::Low, |_| {
            info!("event: game_started");
        });
        events::subscribe(kind::MOD_LOADED, Priority::Normal, |_| {
            info!("event: mod_loaded");
        });
        events::subscribe(kind::CLIENT_TICK, Priority::Monitor, |_| {
            if self_ticks() < 3 {
                info!("event: client_tick #{}", self_ticks());
            }
        });
        events::subscribe_once(kind::CLIENT_TICK, Priority::Highest, |_| {
            info!("event: first client_tick");
        });
        events::subscribe(kind::RENDER_TICK, Priority::Low, |event| {
            if self_ticks() < 3 {
                info!("event: render_tick flag={:?}", event.flag());
            }
        });
        events::subscribe(kind::INPUT, Priority::Highest, |event| {
            info!("event: input {} down={:?}", event.text().unwrap_or("?"), event.flag());
            if event.text() == Some("hello_mod:panic") {
                events::stop_propagation();
                info!("event: propagation stopped before the lower priorities");
            }
        });
        events::subscribe_filtered(
            kind::MOD_DISABLED,
            Priority::Normal,
            |_| true,
            |event| {
                info!(
                    "event: mod_disabled for {}, running again={}",
                    event.text().unwrap_or("?"),
                    mods::is_enabled("hello_mod"),
                );
            },
        );
        events::subscribe(kind::MOD_ENABLED, Priority::Normal, |event| {
            info!("event: mod_enabled for {}", event.text().unwrap_or("?"));
        });
        events::subscribe("hello:ready", Priority::Normal, |event| {
            info!(
                "event: hello:ready delivered late, carrying {:?} from {}",
                event.text(),
                event.source(),
            );
        });
        let queued = events::subscribe(kind::GAME_TICK_END, Priority::Normal, |event| {
            info!("event: game_tick_end carried {:?}", event.payload());
        });
        info!(
            "events: listeners(CLIENT_TICK)={} order={:?} kinds={} queued_id={queued}",
            events::get_listener_count(kind::CLIENT_TICK),
            events::get_event_order(kind::CLIENT_TICK)
                .into_iter()
                .map(|info| (info.priority.as_str(), info.once))
                .collect::<Vec<_>>(),
            events::known_kinds().len(),
        );
        // Diagnostics read after the loader has finished starting up, which is when they
        // say something worth logging.
        tasks::schedule_after_ticks(16, move || {
            if !demo(8) {
                return;
            }
            info!("diagnostics: environment={:?}", diagnostics::get_environment());
            info!("diagnostics: runtime={:?}", diagnostics::get_runtime_info());
            info!("diagnostics: game={:?}", diagnostics::get_game_state());
            info!("diagnostics: loader={:?}", diagnostics::get_loader_state());
            info!("diagnostics: mod_status={:?}", diagnostics::get_mod_status());
            info!("diagnostics: memory={:?}", diagnostics::get_memory_usage());
            info!("diagnostics: threads={:?}", diagnostics::get_active_threads());
            info!("diagnostics: errors={}", diagnostics::get_recent_errors().len());
            for row in diagnostics::get_capability_report()
            .into_iter()
            .filter(|row| row.declared)
        {
                    info!("diagnostics: capability {row}");
        }
            diagnostics::note("load finished");
            info!("diagnostics: timeline={:?}", diagnostics::get_timeline());
            info!("diagnostics: report_bytes={}", diagnostics::report().len());
            match diagnostics::export_report(None) {
                Ok(path) => info!("diagnostics: report written to {}", path.display()),
                Err(error) => warn!("diagnostics: could not write the report: {error}"),
        }

        });

        // A posted event waits for the end of the tick; a published one runs now.
        events::post(events::Event::new("hello:ready").with_text("queued during load"));

        // Section 8: the log this mod owns, and what a report says about it.
        log::capture_panics();
        log::capture_errors(true);
        log::set_level(Level::Debug);
        let sink = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = std::sync::Arc::clone(&sink);
        log::register_sink_with_flush(
            "hello:tally",
            move |record: &Record| {
                collected.lock().unwrap().push(record.render());
            },
            Some(|| info!("log: the tally sink was flushed")),
        );
        log::set_filter("hello_mod", Level::Trace);
        log::trace("log: trace through the filter");
        log::debug("log: debug line");
        log::info("log: info line");
        log::warn("log: warn line");
        log::error("log: error line, kept for the report");
        log::fatal("log: fatal line, kept whether capture is on or not");
        log::event("hello:beat", &[("tick", "0"), ("why", "load")]);
        log::with_context("phase=load", || log::debug("log: inside a context"));
        log::report_deprecation("meta!()", "export_mod!(T, manifest = ...)");
        log::report_warning("the example log is loud", "section 8 is the point");
        log::report_exception("ExampleError", "the example threw");
        log::get_structured_logger("hello:assets").structured(
            Level::Info,
            "log: structured",
            &[("files", "3"), ("bytes", "4096")],
        );
        info!(
            "log: level={} filters={:?} sinks={:?} recent={} counters={:?} enabled(trace)={}",
            log::get_level(),
            log::get_filters(),
            log::sinks(),
            log::get_recent(64).len(),
            log::counters(),
            log::is_enabled(Level::Trace),
        );
        info!(
            "log: format_error={:?}",
            log::format_error(&std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
        );
        info!("log: flushed {} sink(s)", log::flush());
        // Section 7: work for later, work off the thread, and the rules for both.
        tasks::submit(|| info!("tasks: ran at the end of the tick"));
        self.beat_task = tasks::schedule_repeating(Duration::from_millis(500), || {
            let count = REPEATS.fetch_add(1, Ordering::Relaxed) + 1;
            info!(
                "tasks: repeat #{count} on {} thread={}",
                tasks::current_thread(),
                tasks::is_main_thread(),
            );
        });
        tasks::schedule_after(Duration::from_secs(3), || info!("tasks: three seconds in"));
        tasks::schedule_after_ticks(30, || info!("tasks: thirty ticks in"));
        tasks::schedule_render_thread(|| {
            info!(
                "tasks: render task in phase {} on {}",
                current_phase(),
                tasks::current_thread(),
            );
        });
        let never = tasks::schedule_after(Duration::from_secs(600), || {
            info!("tasks: this must never run")
        });
        info!(
            "tasks: cancel(never)={} status={:?}",
            tasks::cancel(never),
            tasks::get_status(never),
        );
        self.worker = Some(tasks::submit_with_result(|| {
            let sum: u64 = (1..=1_000_000).sum();
            (sum, tasks::current_thread(), tasks::is_main_thread())
        }));
        info!(
            "tasks: queued={} pending={} metrics={:?}",
            tasks::queue_length(),
            tasks::get_pending().len(),
            tasks::get_metrics(),
        );
        info!(
            "tasks: thread={} id={} main={} render={} network={} assert_main={}",
            tasks::current_thread(),
            tasks::thread_id(),
            tasks::is_main_thread(),
            tasks::is_render_thread(),
            tasks::is_network_thread(),
            tasks::assert_main_thread(),
        );
        let refused = tasks::schedule_network_thread(|| {});
        info!(
            "tasks: network refusal={} executor={}",
            refused.is_none(),
            TaskExecutor::Network.submit(|| {}).is_none(),
        );
        let pool = WorkerPool::new(2);
        let pooled: TaskResult<&'static str> = pool.submit_with_result(|| "pool job done");
        info!("tasks: pool size={}", pool.size());
        self.pool_result = Some(pooled);

        self.beat = Timer::every(1000);

        // Everything below reads state and logs it, so it runs after the loader has finished
        // starting up rather than during `load`: a heavy load phase, or a heavy single
        // callback, has tripped the hook layer before.

        // // Section 9: a typed config with a schema, watched and validated. — one group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(2, move || {
            if !demo(9) {
                return;
            }
            // Section 9: a typed config with a schema, watched and validated.
            fn rename_corner(config: &mut std::collections::BTreeMap<String, ConfigValue>) {
                if let Some(value) = config.remove("corner_name") {
                    config.insert("corner_label".to_owned(), value);
                }
            }
            config::register_schema(
                ConfigSchema::new("hello")
                    .version(env!("CARGO_PKG_VERSION"))
                    .migration(ConfigMigration::new(
                        "0.0.1",
                        "0.1.0",
                        "corner_name became corner_label",
                        rename_corner,
                    ))
                    .entry(
                        ConfigEntry::new("greeting", ConfigValue::Text("hello".to_owned()))
                            .comment("what the mod says when it starts")
                            .group(ConfigGroup::new("Text", "Display"))
                            .constraint(config::required())
                            .constraint(config::max_length(32)),
                    )
                    .entry(
                        ConfigEntry::new("loudness", ConfigValue::Percentage(50.0))
                            .comment("how loudly it says it")
                            .constraint(config::range(0.0, 100.0)),
                    )
                    .entry(
                        ConfigEntry::new("target", ConfigValue::String("minecraft:stone".to_owned()))
                            .constraint(config::pattern("minecraft:*"))
                            .constraint(config::one_of(&["minecraft:stone", "minecraft:dirt"])),
                    )
                    .entry(
                        ConfigEntry::new("dev_only", ConfigValue::Boolean(false))
                            .visibility(ConfigVisibility::Development)
                            .restart(ConfigRestartRequirement::Restart),
                    )
                    .entry(
                        ConfigEntry::new("enabled_when_demo", ConfigValue::Boolean(true))
                            .constraint(config::visible_when("loudness", &["100"])),
                    ),
            );
            config::register_validator(ConfigValidator::new("loudness must be a step", |config| {
                match config.get("loudness").and_then(ConfigValue::as_float) {
                    Some(loudness) if (loudness % 5.0).abs() > f64::EPSILON => {
                        vec![format!("loudness {loudness} is not a multiple of 5")]
                    }
                    _ => Vec::new(),
                }
            }));
            let loaded = config::load();
            let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let into = std::sync::Arc::clone(&seen);
            config::watch("greeting", move |key, value| {
                into.lock().unwrap().push(format!("{key}={}", value.render()));
            });
            config::on_changed(|all| info!("config: {} keys now", all.len()));
            info!(
                "config: loaded={loaded} keys={:?} dirty={} errors={:?}",
                config::keys(),
                config::is_dirty(),
                config::get_errors(),
            );
            info!(
                "config: get(greeting)={:?} get_or(missing)={:?} contains(loudness)={}",
                config::get("greeting"),
                config::get_or("missing", ConfigValue::Integer(7)),
                config::contains("loudness"),
            );
            info!(
                "config: set(greeting)={} set(loudness=61)={} set(bad target)={} errors={:?}",
                config::set("greeting", ConfigValue::Text("hi there".to_owned())),
                config::set("loudness", ConfigValue::Percentage(61.0)),
                config::set("target", ConfigValue::String("other:stone".to_owned())),
                config::get_errors(),
            );
            info!("config: watcher saw {:?}", seen.lock().unwrap());
            let mut txn = config::transaction();
            config::set("greeting", ConfigValue::Text("in a transaction".to_owned()));
            info!("config: inside a transaction get={:?}", config::get("greeting"));
            info!("config: rollback={}", txn.rollback());
            info!("config: after rollback get={:?}", config::get("greeting"));
            info!(
                "config: defaults={:?} schema={:?} values={}",
                config::get_defaults().keys().collect::<Vec<_>>(),
                config::get_schema().map(|schema| schema.entries.len()),
                config::values().len(),
            );
            info!(
                "config: reset_key(loudness)={} now={:?}",
                config::reset_key("loudness"),
                config::get("loudness"),
            );
            info!("config: save={} dirty={}", config::save(), config::is_dirty());
            match config::backup() {
                Ok(path) => info!("config: backup at {}", path.display()),
                Err(error) => warn!("config: no backup: {error}"),
            }
            match config::export(None) {
                Ok(path) => info!("config: exported to {}", path.display()),
                Err(error) => warn!("config: no export: {error}"),
            }
            info!("config: reload={} keys={:?}", config::reload(), config::keys());
            match config::restore() {
                Ok(count) => info!("config: restored {count} entries"),
                Err(error) => warn!("config: no restore: {error}"),
            }
            let migration_probe = config::migrate("0.0.1", env!("CARGO_PKG_VERSION"));
            info!(
                "config: migrate ran {migration_probe} step(s), remove(missing)={}, reset={}",
                config::remove("nothing_here"),
                config::reset(),
            );
            info!(
                "config: all={:?} errors={:?} validators={}",
                config::all().keys().collect::<Vec<_>>(),
                config::validate(),
                config::get_errors().len(),
            );
        });

        // // Section 10: what this mod may do, and what it may not. — one group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(4, move || {
            if !demo(10) {
                return;
            }
            // Section 10: what this mod may do, and what it may not.
            info!(
                "permissions: trust={} trusted={} declared={:?}",
                permissions::get_trust_level(),
                permissions::is_trusted(),
                permissions::list_declared(),
            );
            info!(
                "permissions: has(WriteFiles)={} has(ReadFiles)={} has(AccessMemory)={} has(LaunchProcesses)={}",
                permissions::has(Permission::WriteFiles),
                permissions::has(Permission::ReadFiles),
                permissions::has(Permission::AccessMemory),
                permissions::has(Permission::LaunchProcesses),
            );
            info!(
                "permissions: request(RegisterCommands)={} request(AccessMemory)={} reason={:?}",
                permissions::request(Permission::RegisterCommands),
                permissions::request(Permission::AccessMemory),
                permissions::denied()
                    .iter()
                    .find(|(permission, _)| *permission == Permission::AccessMemory)
                    .map(|(_, reason)| reason.clone()),
            );
            info!(
                "permissions: grant(AccessMemory)={} grant(CameraControl)={} revoked(CameraControl)={} restrict(CameraControl)={} grant again={}",
                permissions::grant(Permission::AccessMemory),
                permissions::grant(Permission::OpenUrls),
                permissions::revoke(Permission::OpenUrls),
                permissions::restrict(Permission::OpenUrls),
                permissions::grant(Permission::OpenUrls),
            );
            info!(
                "permissions: prompt_user(ReadGameState)={} set_trust(Trusted)={} set_trust(Untrusted)={} now={}",
                permissions::prompt_user(Permission::ReadGameState),
                permissions::set_trust_level(TrustLevel::Trusted),
                permissions::set_trust_level(TrustLevel::Untrusted),
                permissions::get_trust_level(),
            );
            permissions::record_access(Permission::ReadFiles, "read the config at load");
            info!(
                "permissions: list={} policy={:?} for core_lib={:?} audit={} lines",
                permissions::list().len(),
                permissions::get_policy()
                    .iter()
                    .filter(|(permission, _)| *permission == Permission::AccessMemory
                        || *permission == Permission::ReadFiles)
                    .map(|(permission, grant)| (permission.as_str(), grant.to_string()))
                    .collect::<Vec<_>>(),
                permissions::list_for_mod("core_lib"),
                permissions::get_audit_log().len(),
            );
            for line in permissions::report() {
                info!("permissions: {line}");
            }
            info!(
                "permissions: registered={} unregister={}",
                permissions::register(Permission::OpenUrls),
                permissions::unregister(Permission::OpenUrls),
            );
        });

        // // Section 11: the mod's own files, and a key/value store beside its config. — one group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(6, move || {
            if !demo(11) {
                return;
            }
            // Section 11: the mod's own files, and a key/value store beside its config.
            info!(
                "fs: root={:?} create={:?} write={:?}",
                fs::root(),
                fs::create_directories("cache"),
                fs::write_text("cache/last-run.txt", "hello from the example"),
            );
            info!(
                "fs: exists={} is_file={} is_directory={} text={:?} bytes={:?}",
                fs::exists("cache/last-run.txt"),
                fs::is_file("cache/last-run.txt"),
                fs::is_directory("cache"),
                fs::read_text("cache/last-run.txt"),
                fs::read_bytes("cache/last-run.txt").map(|bytes| bytes.len()),
            );
            info!("fs: append={:?}", fs::append("cache/last-run.txt", b" again"));
            info!(
                "fs: escape refusals: {:?} {:?}",
                fs::read_text("../outside.txt").err(),
                fs::write_text("/etc/cinnaloader-probe", "no"),
            );
            info!(
                "fs: copy={:?} rename={:?} size={} modified={:?}",
                fs::copy("cache/last-run.txt", "cache/second.txt", true),
                fs::rename("cache/second.txt", "cache/third.txt"),
                fs::file_size("cache/third.txt"),
                fs::modified_time("cache/third.txt").is_some(),
            );
            info!(
                "fs: list={:?} walk={} metadata={:?}",
                fs::list("cache").map(|entries| entries.len()),
                fs::walk("cache").map(|entries| entries.len()).unwrap_or_default(),
                fs::metadata("cache/last-run.txt").map(|meta| (meta.size, meta.is_file, meta.age().is_some())),
            );
            info!(
                "fs: canonical={:?} hash={:?} identical={} delete={:?}",
                fs::canonicalize("cache/third.txt"),
                fs::hash("cache/third.txt"),
                fs::identical("cache/last-run.txt", "cache/third.txt"),
                fs::delete("cache/third.txt"),
            );
            info!(
                "fs: atomic_write={:?} read back={:?}",
                fs::atomic_write("cache/atomic.txt", b"written atomically"),
                fs::read_text("cache/atomic.txt"),
            );
            let mut handle = fs::open_write("cache/streamed.txt").unwrap_or_else(|_| {
                fs::open_read("cache/atomic.txt").expect("the file that was just written")
            });
            let _ = handle.write(b"streamed through a handle");
            let _ = handle.flush();
            info!(
                "fs: handle path={} read back={:?}",
                handle.path().display(),
                fs::read_text("cache/streamed.txt"),
            );
            info!(
                "fs: temp file={:?} temp dir={:?} watch={:?} watch_count={} unwatch={}",
                fs::temporary_file(Some("cache")).map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
                fs::temporary_directory(Some("cache")).map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
                fs::watch("cache/atomic.txt", |path, _| info!("fs: watched {} change", path.display())).map(|_| ()),
                fs::watch_count(),
                fs::unwatch(1),
            );
            let packed = fs::compress(b"aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbcc");
            info!(
                "fs: compressed {} bytes to {} and back to {:?}",
                34,
                packed.len(),
                fs::decompress(&packed).map(|bytes| bytes.len()),
            );
            info!(
                "storage: open={:?} set={} flush={:?} keys={:?}",
                storage::open("seen"),
                storage::set("blocks", ConfigValue::Integer(42)),
                storage::flush(),
                storage::keys(""),
            );
            let world = storage::namespace("world");
            world.set("spawn", ConfigValue::Text("0,64,0".to_owned()));
            info!(
                "storage: version={:?} get(blocks)={:?} namespaced={:?} all={}",
                storage::version(),
                storage::get("blocks"),
                world.get("spawn"),
                storage::keys("").len(),
            );
            let mut batch = storage::transaction().unwrap();
            storage::set("blocks", ConfigValue::Integer(7));
            info!("storage: inside a batch blocks={:?} rollback={}", storage::get("blocks"), batch.rollback());
            info!("storage: after rollback blocks={:?}", storage::get("blocks"));
            storage::set("blocks", ConfigValue::Integer(43));
            info!(
                "storage: migrate={:?} version={:?} delete={} contains={} clear={}",
                storage::migrate("2", |values| {
                    if let Some(value) = values.remove("old") {
                        values.insert("renewed".to_owned(), value);
                    }
                }),
                storage::version(),
                storage::delete("nothing"),
                storage::contains("blocks"),
                storage::clear(),
            );
            storage::set("blocks", ConfigValue::Integer(44));
            info!(
                "storage: flush={:?} backup={:?} restore={:?} stores={:?} close={} open_database={:?}",
                storage::flush(),
                storage::backup().map(|path| path.file_name().map(|name| name.to_string_lossy().into_owned())),
                storage::restore(),
                storage::stores(),
                storage::close(),
                storage::open_database("log"),
            );
            info!(
                "storage: packed open={:?} set={} flush={:?} close={}",
                storage::open("tidy.packed"),
                storage::set("note", ConfigValue::Text("packed".to_owned())),
                storage::flush(),
                storage::close(),
            );
            info!(
                "storage: format round trip get={:?} stores={:?}",
                storage::open("tidy.packed").map(|_| storage::get("note")),
                storage::stores(),
            );
            storage::close();
        });

        // // Section 12: resources by id, over a stack of packs. — one group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(8, move || {
            if !demo(12) {
                return;
            }
            // Section 12: resources by id, over a stack of packs.
            let mut own = ResourcePack::memory("hello-demo", 5).namespace("hello");
            own.insert("textures/gui/menu.png", b"png bytes".to_vec());
            own.insert("lang/en_us.txt", b"greeting=hi there".to_vec());
            resources::register_pack(own);
            let mut weaker = ResourcePack::memory("hello-weaker", 1).namespace("hello");
            weaker.insert("lang/en_us.txt", b"greeting=low priority".to_vec());
            resources::register_pack(weaker);
            let mut overlay = ResourcePack::memory("hello-overlay", 20).namespace("hello");
            overlay.insert("data/thing.json", b"{\"overlay\":true}".to_vec());
            resources::register_pack(overlay);
            info!(
                "resources: registered={} packs={:?} stack={:?}",
                resources::register(
                    &"hello:textures/extra.png".parse().unwrap_or_else(|_| unreachable!()),
                    b"registered at runtime".to_vec()
                ),
                resources::get_active_packs(),
                resources::get_resource_stack(),
            );
            let menu = "hello:textures/gui/menu.png".parse::<ResourceId>().ok();
            let lang = "hello:lang/en_us.txt".parse::<ResourceId>().ok();
            info!(
                "resources: exists={:?} text={:?} bytes={:?} metadata={:?}",
                menu.as_ref().map(resources::exists),
                lang.as_ref().and_then(resources::read_text),
                lang.as_ref().and_then(resources::read_bytes).map(|bytes| bytes.len()),
                menu.as_ref().and_then(resources::metadata),
            );
            info!(
                "resources: list={} find={} overrides={:?}",
                resources::list("hello").len(),
                resources::find("hello:lang/*").len(),
                resources::get_overrides()
                    .into_iter()
                    .map(|over| over.id.to_string())
                    .collect::<Vec<_>>(),
            );
            info!(
                "resources: namespaces={:?} priorities={:?} set_priority={} now={:?}",
                resources::get_namespace(&lang.clone().unwrap()),
                [
                    resources::get_priority("hello-overlay"),
                    resources::get_priority("hello-demo"),
                ],
                resources::set_priority("hello-weaker", 30),
                resources::get_priority("hello-weaker"),
            );
            let fired = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counter = std::sync::Arc::clone(&fired);
            resources::add_reload_listener("hello:cache", &["hello"], move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
            resources::watch(&lang.clone().unwrap(), |id| info!("resources: watched {id} reload"));
            info!(
                "resources: reload={} namespace reload={} listeners={:?} fired={} count={}",
                resources::reload(),
                resources::reload_namespace("hello"),
                resources::get_reload_listeners(),
                fired.load(std::sync::atomic::Ordering::Relaxed),
                resources::reload_count(),
            );
            resources::depend(ResourceDependency {
                id: "hello:textures/absent.png".parse().unwrap(),
                required_by: "hello-mod".to_owned(),
                optional: false,
            });
            info!(
                "resources: validate={:?} missing={:?} errors={:?}",
                resources::validate(),
                resources::get_missing_resources()
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>(),
                resources::get_loading_errors().len(),
            );
            info!(
                "resources: own_pack={:?} assets={:?} unregister={:?} unregister_pack={:?}",
                resources::own_pack().map(|pack| pack.name),
                resources::assets_directory().is_some(),
                menu.as_ref().map(resources::unregister),
                resources::unregister_pack("hello-overlay"),
            );
        });

        // // Section 13: keys in the languages this mod ships. — one group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(10, move || {
            if !demo(13) {
                return;
            }
            // Section 13: keys in the languages this mod ships.
            fs::write_text(
                "assets/lang/en_us.txt",
                "hello=Hello, {0}!\napples=one apple|{0} apples\nbye=Bye\n",
            )
            .ok();
            fs::write_text(
                "assets/lang/de_de.txt",
                "hello=Hallo, {0}!\napples=ein Apfel|{0} Äpfel\n",
            )
            .ok();
            resources::register_pack(
                ResourcePack::directory("hello-mod-assets", "assets", 50).namespace("hello"),
            );
            i18n::register_language("en_us", "English");
            i18n::register_language("de_de", "Deutsch");
            info!(
                "i18n: languages={:?} current={} fallback={} read={}",
                i18n::get_languages()
                    .into_iter()
                    .map(|language| language.code)
                    .collect::<Vec<_>>(),
                i18n::get_current_language(),
                i18n::get_fallback_language(),
                i18n::reload(),
            );
            let heard = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counter = std::sync::Arc::clone(&heard);
            i18n::on_language_changed(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
            info!(
                "i18n: set(de_de)={} current={} translate={:?} with_args={:?} plural1={:?} plural4={:?}",
                i18n::set_current_language("de_de"),
                i18n::get_current_language(),
                i18n::translate("hello"),
                i18n::translate_with("hello", &["Welt"]),
                i18n::translate_plural("apples", 1),
                i18n::translate_plural("apples", 4),
            );
            info!(
                "i18n: has(hello)={} has(bye)={} get(bye)={:?} in_de={:?} watchers={}",
                i18n::has_translation("hello"),
                i18n::has_translation("bye"),
                i18n::get_translation("bye"),
                i18n::get_translation_in("de_de", "hello"),
                i18n::watcher_count(),
            );
            i18n::set_fallback_language("en_us");
            info!(
                "i18n: formats number={:?} number_de={:?} date={:?} time={:?} duration={:?} currency={:?}",
                i18n::format_number(1234567.5),
                i18n::format_number_in("de_de", 1234567.5),
                i18n::format_date(1_700_000_000),
                i18n::format_time(86_400 + 3661),
                i18n::format_duration(3_723_000),
                i18n::format_currency(12.5, "usd"),
            );
            i18n::register_translations("fr_fr", &[("hello", "Bonjour, {0} !")]);
            info!(
                "i18n: unregister_translations(fr_fr)={} unregister_language(fr_fr)={} languages={} resource_id={:?}",
                i18n::unregister_translations("fr_fr"),
                i18n::unregister_language("fr_fr"),
                i18n::get_languages().len(),
                i18n::resource_id_for("en_us").map(|id| id.to_string()),
            );
        });

        // // Section 14: what the game is doing, as far as this host can honestly tell. — one group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(12, move || {
            if !demo(14) {
                return;
            }
            // Section 14: what the game is doing, as far as this host can honestly tell.
            info!(
                "game: running={} version={:?} protocol={:?} multiplayer={} singleplayer={} server={:?} online={}",
                game::is_running(),
                game::get_version(),
                game::get_protocol_version(),
                game::is_multiplayer(),
                game::is_singleplayer(),
                game::current_server(),
                game::is_online(),
            );
            info!(
                "game: phase={} tick={} frames={} uptime={}s fps={:?} tps={:?} window={:?}",
                game::current_phase(),
                game::current_tick(),
                game::frames(),
                game::uptime_secs(),
                game::current_fps(),
                game::current_tps(),
                game::get_window(),
            );
            info!(
                "game: session={:?} runtime={:?}",
                game::get_session_info(),
                game::get_runtime_info(),
            );
            info!(
                "game: settings={:?} registry={} resources={}",
                game::get_settings(),
                game::get_registry_manager().len(),
                game::get_resource_manager().len(),
            );
            info!(
                "game: unseen: screen={:?} world={:?} player={:?} dimension={:?} mode={:?} paused={:?} menu={:?} in_world={:?} time={:?}",
                game::current_screen(),
                game::current_world(),
                game::current_player(),
                game::current_dimension(),
                game::current_game_mode(),
                game::is_paused(),
                game::is_in_menu(),
                game::is_in_world(),
                game::current_time(),
            );
            info!(
                "game: unreachable: client={:?} server={:?} network={:?} renderer={:?} why={:?}",
                game::get_client(),
                game::get_server(),
                game::get_network_manager(),
                game::get_world_renderer(),
                game::why_unknown("world"),
            );
            info!(
                "game: requests: disconnect={} world_reload={} resource_reload={} shutdown={} log={:?}",
                game::request_disconnect(),
                game::request_world_reload(),
                game::request_resource_reload(),
                game::request_game_shutdown(),
                game::get_requests()
                    .into_iter()
                    .map(|outcome| format!("{}: {}", outcome.request, outcome.granted))
                    .collect::<Vec<_>>(),
            );
            info!("game: host clock={} slowest_frame={}ms", game::now_unix_seconds(), game::slowest_frame_millis());

            // A hook that cannot run yet, and one that runs as this mod is dropped.
            defer_until_ready(|| info!("lifecycle: deferred hook ran once the loader was ready"));
            register_cleanup(|| info!("lifecycle: cleanup ran while unloading"));
            // Whatever the user last chose, kept next to the mods.
            if let Some(name) = config_value("name") {
                info!("hello-mod remembered {name}");
            } else {
                set_config("name", "hello-mod");
            }
            info!("hello-mod initialised (mods from {})", mods_dir().unwrap_or("?"));
        });

        // Section 15: the world (which this host cannot see) and the dimensions a mod can
        // declare (which it can). One group per tick: every section doing its work inside one
        // hook callback at once is what trips the hook layer (see AGENTS.md).
        tasks::schedule_after_ticks(14, move || {
            if !demo(1014) {
                return;
            }
            info!(
                "world: get={:?} by_id={:?} by_name={:?} all={} loaded={:?}",
                world::get(),
                world::get_by_id("overworld"),
                world::get_by_name("World"),
                world::get_all().len(),
                world::is_loaded(),
            );
            info!(
                "world: load={} unload={} refusals={:?}",
                world::load("overworld"),
                world::unload(),
                world::refusals()
                    .into_iter()
                    .map(|refusal| refusal.what)
                    .collect::<Vec<_>>(),
            );
            info!(
                "world: id={:?} name={:?} seed={:?} time={:?} day_time={:?} game_time={:?} difficulty={:?}",
                world::known_only_to_the_client::get_id(),
                world::known_only_to_the_client::get_name(),
                world::known_only_to_the_client::get_seed(),
                world::known_only_to_the_client::get_time(),
                world::known_only_to_the_client::get_day_time(),
                world::known_only_to_the_client::get_game_time(),
                world::known_only_to_the_client::get_difficulty(),
            );
            info!(
                "world: rules={:?} spawn={:?} set_spawn={} border={:?} height={:?}..{:?} chunks={}",
                world::known_only_to_the_client::get_game_rules(),
                world::known_only_to_the_client::get_spawn_position(),
                world::known_only_to_the_client::set_spawn_position(0.0, 64.0, 0.0),
                world::known_only_to_the_client::get_border(),
                world::known_only_to_the_client::get_min_height(),
                world::known_only_to_the_client::get_max_height(),
                world::known_only_to_the_client::get_chunks().len(),
            );
            info!(
                "world: chunk={:?} loaded={:?} load={} request={} players={} entities={} in_range={} in_box={} blocks={}",
                world::known_only_to_the_client::get_chunk(0, 0),
                world::known_only_to_the_client::is_chunk_loaded(0, 0),
                world::known_only_to_the_client::load_chunk(0, 0),
                world::known_only_to_the_client::request_chunk(0, 0),
                world::known_only_to_the_client::get_players().len(),
                world::known_only_to_the_client::get_entities().len(),
                world::known_only_to_the_client::get_players_in_range(0.0, 64.0, 0.0, 16.0).len(),
                world::known_only_to_the_client::get_entities_in_box((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)).len(),
                world::known_only_to_the_client::get_blocks_in_box((0, 0, 0), (1, 1, 1)).len(),
            );
            info!(
                "world: weather={:?} set_weather={} biome={:?} dimension={:?} save_path={:?} metadata={:?} save={} dirty={} snapshot={:?} restore={}",
                world::known_only_to_the_client::get_weather(),
                world::known_only_to_the_client::set_weather("rain"),
                world::known_only_to_the_client::get_biome(0, 64, 0),
                world::known_only_to_the_client::get_dimension(),
                world::known_only_to_the_client::get_save_path(),
                world::known_only_to_the_client::get_metadata(),
                world::known_only_to_the_client::save(),
                world::known_only_to_the_client::mark_dirty(),
                world::known_only_to_the_client::create_snapshot(),
                world::known_only_to_the_client::restore_snapshot(1),
            );
            info!(
                "dimensions: register={} get={:?}",
                dimensions::register(
                    DimensionDescription::new("skylands", "Skylands")
                        .kind(DimensionKind::Custom)
                        .bounds(-64, 320)
                        .gravity(0.6)
                        .time(12_000)
                        .biome_source("minecraft:sky")
                        .ambient(AmbientProperties {
                            light: 0.8,
                            fog_color: 0x00a0c0ff,
                            sky_color: 0x0070a0ff,
                            has_sky: true,
                            has_weather: false,
                        })
                        .respawn(RespawnRules {
                            dimension: "inkwell".to_owned(),
                            world_spawn: true,
                            respawn_anchor_works: true,
                        })
                ),
                dimensions::get("skylands").map(|description| description.to_string()),
            );
            info!(
                "dimensions: all={} by_name={:?} by_id={:?} type={:?} bounds={:?} gravity={:?} time={:?}",
                dimensions::get_all().len(),
                dimensions::get_by_name("Skylands").map(|description| description.name),
                dimensions::get_by_id("skylands").map(|description| description.id),
                dimensions::get_type("skylands"),
                dimensions::get_bounds("skylands"),
                dimensions::get_gravity("skylands"),
                dimensions::get_time("skylands"),
            );
            info!(
                "dimensions: biome={:?} ambient={:?} respawn={:?} created={} create={} teleport={}",
                dimensions::get_biome_source("skylands"),
                dimensions::get_ambient_properties("skylands"),
                dimensions::get_respawn_rules("skylands").map(|rules| rules.dimension),
                dimensions::is_created("skylands"),
                dimensions::create("skylands"),
                dimensions::teleport_to_dimension("skylands"),
            );
            info!("dimensions: unregister={}", dimensions::unregister("skylands"));
        });

        // Section 16: the blocks a mod declares, and the world it cannot reach.
        tasks::schedule_after_ticks(16, move || {
            if !demo(16) {
                return;
            }
            blocks::register_event_kinds();
            info!(
                "blocks: register={} events={} known={}",
                blocks::register(
                    BlockDescription::new("lantern", "Lantern")
                        .material(BlockMaterial::Metal)
                        .hardness(1.5)
                        .resistance(2.0)
                        .light_level(12)
                        .transparent(true)
                        .property(BlockProperty::facing(&["north", "south", "east", "west"]))
                        .property(BlockProperty::waterlogged())
                        .tool("pickaxe", "iron", true)
                        .drops(&["lantern"])
                        .block_entity(true)
                        .behaviour(BlockBehaviour {
                            on_break: Some(|| {}),
                            on_use: Some(|| {}),
                            ..BlockBehaviour::default()
                        })
                ),
                blocks::event_kinds().len(),
                blocks::has_event_kind("block_break"),
            );
            info!(
                "blocks: type={:?} by_name={:?} id={:?} registry_id={:?} hardness={:?} resistance={:?} friction={:?} sound={:?}",
                blocks::get_type("lantern").map(|block| block.to_string()),
                blocks::get_by_name("Lantern").map(|block| block.name),
                blocks::get_id("lantern"),
                blocks::get_registry_id("lantern"),
                blocks::get_hardness("lantern"),
                blocks::get_resistance("lantern"),
                blocks::get_friction("lantern"),
                blocks::get_sound("lantern"),
            );
            info!(
                "blocks: properties={:?} light={:?} solid={:?} transparent={:?} fluid={:?} air={:?} drops={:?} tool={:?}",
                blocks::get_properties("lantern")
                    .into_iter()
                    .map(|property| (property.name, property.values.len(), property.default))
                    .collect::<Vec<_>>(),
                blocks::get_light_level("lantern"),
                blocks::is_solid("lantern"),
                blocks::is_transparent("lantern"),
                blocks::is_fluid("lantern"),
                blocks::is_air("lantern"),
                blocks::get_drops("lantern"),
                blocks::get_tool_requirement("lantern").map(|tool| (tool.tool, tool.tier, tool.required)),
            );
            info!(
                "blocks: shapes={:?} {:?} fluid_state={:?} entity={:?} behaviour={:?} why={:?}",
                blocks::get_collision_shape("lantern"),
                blocks::get_render_shape("lantern"),
                blocks::get_fluid_state("lantern"),
                blocks::get_type("lantern").map(|block| block.block_entity),
                blocks::behaviour::callbacks("lantern"),
                blocks::behaviour::why_never_called(),
            );
            info!(
                "blocks: world at={:?} state={:?} type={:?} light={:?} entity={:?} place={:?}",
                blocks::get_at(0, 64, 0),
                blocks::get_state(0, 64, 0),
                blocks::get_type_at(0, 64, 0),
                blocks::get_light_level_at(0, 64, 0),
                blocks::get_block_entity_at(0, 64, 0),
                blocks::can_place_at(0, 64, 0, "lantern"),
            );
            info!(
                "blocks: set={} place={} break={} remove={} replace={} update={} neighbors={} tick={} cancel={}",
                blocks::set(0, 64, 0, "lantern"),
                blocks::place(0, 64, 0, "lantern"),
                blocks::break_block(0, 64, 0),
                blocks::remove(0, 64, 0),
                blocks::replace(0, 64, 0, "lantern", "air"),
                blocks::update(0, 64, 0),
                blocks::notify_neighbors(0, 64, 0),
                blocks::schedule_tick(0, 64, 0, 20),
                blocks::cancel_scheduled_tick(0, 64, 0),
            );
            info!(
                "blocks: refusals={:?} registered_with_client={} unregister={}",
                blocks::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                blocks::is_registered_with_the_client("lantern"),
                blocks::unregister("lantern"),
            );
        });

        // Section 17: item stacks (real values) and the item registry (the client's).
        tasks::schedule_after_ticks(18, move || {
            if !demo(17) {
                return;
            }
            items::register_event_kinds();
            let mut stack = items::create_stack("minecraft:iron_pickaxe", 1).with_max_count(1);
            stack.set_component(items::ItemComponent::new("minecraft:durability", "250"));
            stack.set_damage(50);
            stack.set_name("Digger");
            stack.set_lore(&["made in the example", "still sharp"]);
            stack.add_enchantment("minecraft:efficiency", 3);
            info!(
                "items: stack={stack} count={} damage={:?} durability={:?} max_damage={:?} enchanted={} plain={}",
                stack.count(),
                items::get_damage(&stack),
                stack.durability(),
                items::get_max_damage(&stack),
                items::is_enchanted(&stack),
                stack.is_plain(),
            );
            let split = items::split_stack(&mut stack, 1);
            info!(
                "items: split={:?} left={} merged={:?}",
                split.map(|split| split.count()),
                stack.count(),
                items::merge_stacks(&mut stack, &items::create_stack("minecraft:iron_pickaxe", 3)),
            );
            info!(
                "items: metadata={} name={:?} display={:?} lore={:?} components={:?}",
                items::get_metadata(&stack),
                items::get_name(&stack),
                items::get_display_name(&stack),
                items::get_lore(&stack),
                items::components_of(&stack)
                    .into_iter()
                    .map(|component| component.name)
                    .collect::<Vec<_>>(),
            );
            info!(
                "items: register={} type={:?}",
                items::register(
                    ItemType::new("lantern", "Lantern")
                        .max_count(16)
                        .durability(64)
                        .icon("hello:lantern")
                        .tag("cinnaloader:lights")
                        .component(items::ItemComponent::custom_name("Lantern"))
                        .use_action("place", 10)
                        .cooldown(5)
                        .food(&[items::ItemComponent::new("minecraft:nutrition", "4")])
                        .armour(&[items::ItemComponent::new("minecraft:armor_slot", "head")])
                        .tool(&[items::ItemComponent::new("minecraft:mining_speed", "2")])
                ),
                items::get("lantern").map(|item| item.to_string()),
            );
            info!(
                "items: by_name={:?} registry_id={:?} max_stack={:?} durability={:?} damageable={:?} stackable={:?} edible={:?} equippable={:?}",
                items::get_by_name("Lantern").map(|item| item.name),
                items::get_registry_id("lantern"),
                items::get_max_stack_size("lantern"),
                items::get_durability("lantern"),
                items::is_damageable("lantern"),
                items::is_stackable("lantern"),
                items::is_edible("lantern"),
                items::is_equippable("lantern"),
            );
            info!(
                "items: tags={:?} has_tag={} use={:?}/{:?} cooldown={:?} texture={:?} food={} armour={} tool={}",
                items::get_tags("lantern"),
                items::has_tag("lantern", "cinnaloader:lights"),
                items::get_use_action("lantern"),
                items::get_use_duration("lantern"),
                items::get_cooldown("lantern"),
                items::get_texture("lantern"),
                items::get_food_properties("lantern").len(),
                items::get_armour_properties("lantern").len(),
                items::get_tool_properties("lantern").len(),
            );
            info!(
                "items: declared_stack={:?} events={} known={}",
                items::get("lantern").map(|item| item.stack(3).to_string()),
                items::event_kinds().len(),
                items::has_event_kind("item_use"),
            );
            info!(
                "items: client-side: hand={:?} model={:?} stone={:?} refusals={:?} registered_with_client={}",
                items::get_stack(),
                items::get_model("minecraft:stone"),
                items::get("minecraft:stone"),
                items::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                items::is_registered_with_the_client("lantern"),
            );
            info!("items: unregister={}", items::unregister("lantern"));
        });

        // Section 18: an inventory this host owns, and the live one it does not.
        tasks::schedule_after_ticks(20, move || {
            if !demo(18) {
                return;
            }
            inventory::set_local_inventory(Inventory::player());
            info!(
                "inventory: size={} slots={:?}..{:?} main={:?}..{:?} offhand={:?} armour={:?}..{:?} kinds={:?}",
                inventory::get_size(),
                inventory::HOTBAR_FIRST,
                inventory::HOTBAR_LAST,
                inventory::MAIN_FIRST,
                inventory::MAIN_LAST,
                inventory::OFFHAND,
                inventory::ARMOUR_FIRST,
                inventory::ARMOUR_LAST,
                [
                    inventory::slot_kind(0).as_str(),
                    inventory::slot_kind(20).as_str(),
                    inventory::slot_kind(36).as_str(),
                    inventory::slot_kind(38).as_str(),
                    inventory::slot_kind(50).as_str(),
                ],
            );
            info!(
                "inventory: set={:?} add={:?} count={} contains={} amount={} find={:?} all={:?} first_empty={:?}",
                inventory::set_slot(0, Some(ItemStack::new("minecraft:stone", 64))),
                inventory::add_item(&ItemStack::new("minecraft:stone", 100)),
                inventory::count("minecraft:stone"),
                inventory::contains("minecraft:stone"),
                inventory::contains_amount("minecraft:stone", 164),
                inventory::find("minecraft:stone"),
                inventory::find_all("minecraft:stone"),
                inventory::first_empty_slot(),
            );
            info!(
                "inventory: can_insert={} can_extract={} insert={:?} contents={:?}",
                inventory::can_insert(&ItemStack::new("minecraft:dirt", 1)),
                inventory::can_extract(0),
                inventory::insert(0, &ItemStack::new("minecraft:stone", 8)),
                inventory::get_contents().len(),
            );
            info!(
                "inventory: split={:?} merge={:?} move={:?} swap={:?} extract={:?}",
                inventory::split_stack(0, 20).map(|stack| stack.count()),
                inventory::merge_stacks(3, 0),
                inventory::move_stack(0, 20).map(|stack| stack.count()),
                inventory::swap(0, 1),
                inventory::extract(1).map(|stack| stack.count()),
            );
            let revision = inventory::get_revision();
            inventory::begin_transaction();
            inventory::clear().ok();
            info!(
                "inventory: transaction open={} after clear={} rollback={} count={} revision={}",
                inventory::get_transaction(),
                inventory::count("minecraft:stone"),
                inventory::rollback_transaction(),
                inventory::count("minecraft:stone"),
                inventory::get_revision() > revision,
            );
            inventory::lock();
            info!(
                "inventory: locked={:?} unlocked set={:?}",
                inventory::set_slot(0, None),
                {
                    inventory::unlock();
                    inventory::set_slot(0, None)
                },
            );
            info!(
                "inventory: remove={:?} size={} contents={}",
                inventory::remove_item("minecraft:stone", 1000),
                inventory::get_size(),
                inventory::get_contents().len(),
            );
            info!(
                "inventory: live: get={:?} selected={:?} set_selected={} armor={:?} offhand={:?} cursor={:?} set_cursor={}",
                inventory::get().is_some(),
                inventory::get_selected_slot(),
                inventory::set_selected_slot(3),
                inventory::get_armor().len(),
                inventory::get_offhand(),
                inventory::get_cursor_stack(),
                inventory::set_cursor_stack(None),
            );
            info!(
                "containers: register={} type={:?}",
                inventory::register_type(
                    ContainerType::new("crate", "Crate", 27)
                        .property("burn_time")
                        .title("A crate")
                        .distance_limit(4.0)
                        .on_slot_click()
                        .on_property_change()
                ),
                inventory::get_type("crate").map(|container| container.to_string()),
            );
            info!(
                "containers: types={} handler={} rule={}",
                inventory::get_all_types().len(),
                inventory::register_screen_handler("crate"),
                inventory::register_slot_rule("crate", 0, "cinnaloader:lights"),
            );
            info!(
                "containers: open={:?} close={} of={:?} slots={:?} properties={:?} viewers={:?} interact={:?} distance={:?} title={:?}",
                inventory::open("crate"),
                inventory::close(),
                inventory::container_of("crate"),
                inventory::get_slots(),
                inventory::get_properties(),
                inventory::get_viewers(),
                inventory::can_interact(),
                inventory::get_distance_limit(),
                inventory::get_title(),
            );
            info!(
                "containers: click={} property_change={} refusals={:?} unregister={}",
                inventory::on_slot_click(0, 0),
                inventory::on_property_change("burn_time", 1),
                inventory::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                inventory::unregister_type("crate"),
            );
        });

        // Section 19: the entity types a mod declares, and the client's entity world.
        tasks::schedule_after_ticks(22, move || {
            if !demo(19) {
                return;
            }
            entities::register_event_kinds();
            info!(
                "entities: register={} type={:?}",
                entities::register_type(
                    EntityType::new("wisp", "Wisp")
                        .family("mob")
                        .family("undead")
                        .size(0.5, 0.5)
                        .health(8.0)
                        .movement(0.3)
                        .attribute("minecraft:movement", 0.3)
                        .attribute("minecraft:follow_range", 16.0)
                        .component(EntityComponent::new("minecraft:health", "8"))
                        .tag("cinnaloader:spirits")
                        .rideable(false)
                        .hostile(false)
                ),
                entities::get_by_id("wisp").map(|entity| entity.to_string()),
            );
            info!(
                "entities: by_name={:?} registry_id={:?} max_health={:?} living={:?} player={:?} hostile={:?} rideable={:?}",
                entities::get_by_name("Wisp").map(|entity| entity.name),
                entities::get_registry_id("wisp"),
                entities::get_max_health("wisp"),
                entities::is_living("wisp"),
                entities::is_player("wisp"),
                entities::is_hostile("wisp"),
                entities::is_rideable("wisp"),
            );
            info!(
                "entities: attributes={:?} one={:?} tags={:?} components={:?} has_component={} dimensions={:?} eye={:?}",
                entities::get_attributes("wisp").keys().collect::<Vec<_>>(),
                entities::get_attribute("wisp", "minecraft:movement"),
                entities::get_tags("wisp"),
                entities::get_components("wisp").len(),
                entities::has_component("wisp", "minecraft:health"),
                entities::get_dimensions("wisp"),
                entities::get_eye_height("wisp"),
            );
            info!(
                "entities: box={:?} distance={} squared={} events={} known={} registered_with_client={}",
                entities::get_bounding_box("wisp", 0.0, 64.0, 0.0),
                entities::distance((0.0, 0.0, 0.0), (3.0, 4.0, 0.0)),
                entities::distance_squared((0.0, 0.0, 0.0), (3.0, 4.0, 0.0)),
                entities::event_kinds().len(),
                entities::has_event_kind("entity_spawn"),
                entities::is_registered_with_the_client("wisp"),
            );
            info!(
                "entities: live: get={:?} type={:?} position={:?} velocity={:?} health={:?} alive={:?} sneaking={:?} burning={:?} owning={:?} passengers={:?}",
                entities::get(),
                entities::get_type(),
                entities::get_position(),
                entities::get_velocity(),
                entities::get_live_health(),
                entities::is_alive(),
                entities::is_sneaking(),
                entities::is_burning(),
                entities::get_owner(),
                entities::get_passengers().len(),
            );
            info!(
                "entities: change: spawn={} damage={} teleport={} mount={} glowing={} attribute={}",
                entities::spawn("wisp", 0.0, 64.0, 0.0),
                entities::damage(1.0),
                entities::teleport(0.0, 64.0, 0.0),
                entities::mount("horse"),
                entities::set_glowing(true),
                entities::set_attribute("minecraft:movement", 0.5),
            );
            info!(
                "entities: refusals={:?} unregister={}",
                entities::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                entities::unregister_type("wisp"),
            );
        });

        // Section 20: component and attribute types a mod owns, and an entity's, which it
        // cannot reach.
        tasks::schedule_after_ticks(24, move || {
            if !demo(1024) {
                return;
            }
            info!(
                "components: register={} schema={:?}",
                components::register(
                    ComponentType::new("hello:glow")
                        .described("a light a mod puts on its own entity")
                        .field("colour", ConfigValue::Integer(0xff_ffff))
                        .required_field("strength", ConfigValue::Float(1.0))
                        .validated()
                ),
                components::get_schema("hello:glow")
                    .map(|component| format!("{component} {:?}", component.schema())),
            );
            let mut values = ComponentSet::new();
            let mut glow = components::create("hello:glow");
            glow.set("colour", ConfigValue::Integer(0x00_ff00));
            info!(
                "components: set={} has={} get={:?} names={:?} validate={:?}",
                components::set(&mut values, "hello:glow", glow.clone()),
                components::has(&values, "hello:glow"),
                components::get_value(&values, "hello:glow"),
                components::names_in(&values),
                components::validate("hello:glow", &glow),
            );
            let text = components::serialize(&glow);
            let mut wrong = components::clone_value(&glow);
            wrong.set("colour", ConfigValue::String("green".to_owned()));
            info!(
                "components: serialized={:?} deserialized={:?} serializer={} validator={} refusals={:?}",
                text,
                components::deserialize("hello:glow", &text),
                components::register_serializer("hello:glow"),
                components::register_validator("hello:glow"),
                components::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "components: on an entity: get={:?} set={} remove={} has={:?} query_entities={:?} query_components={:?}",
                components::get_on_entity("wisp", "hello:glow"),
                components::set_on_entity("wisp", "hello:glow", wrong.clone()),
                components::remove_from_entity("wisp", "hello:glow"),
                components::entity_has("wisp", "hello:glow"),
                components::query_entities("hello:glow").len(),
                components::query_components("wisp").len(),
            );
            let mut attributes = AttributeSet::new();
            components::attributes::set(
                &mut attributes,
                Attribute::new("minecraft:movement", 0.25, 0.0, 1.0),
            );
            components::attributes::add_modifier(
                &mut attributes,
                "minecraft:movement",
                AttributeModifier {
                    name: "boots".to_owned(),
                    kind: ModifierKind::Add,
                    amount: 0.05,
                },
            );
            components::attributes::add_modifier(
                &mut attributes,
                "minecraft:movement",
                AttributeModifier {
                    name: "swift".to_owned(),
                    kind: ModifierKind::Multiply,
                    amount: 0.5,
                },
            );
            info!(
                "attributes: base={:?} current={:?} modifiers={:?}",
                components::attributes::get_base_value(&attributes, "minecraft:movement"),
                components::attributes::get_current_value(&attributes, "minecraft:movement"),
                components::attributes::get_modifiers(&attributes, "minecraft:movement")
                    .into_iter()
                    .map(|modifier| modifier.name)
                    .collect::<Vec<_>>(),
            );
            components::attributes::remove_modifier(&mut attributes, "minecraft:movement", "swift");
            info!(
                "attributes: after removing one={:?} modify={} register={} definitions={}",
                components::attributes::get_current_value(&attributes, "minecraft:movement"),
                components::attributes::modify(&mut attributes, "minecraft:movement", 0.9),
                components::attributes::register(AttributeDefinition {
                    name: "hello:focus".to_owned(),
                    owner: "hello_mod".to_owned(),
                    default: 0.0,
                    minimum: 0.0,
                    maximum: 10.0,
                }),
                components::attributes::definitions().len(),
            );
            info!(
                "attributes: live: get={:?} set={} ecs: register_system={} run_system={} order={:?}",
                components::attributes::get_live("wisp", "minecraft:movement"),
                components::attributes::set_live("wisp", "minecraft:movement", 5.0),
                components::register_system("hello:tick"),
                components::run_system("hello:tick"),
                components::get_system_order(),
            );
            info!(
                "components: unregister={} attribute_unregister={}",
                components::unregister("hello:glow"),
                components::attributes::unregister("hello:focus"),
            );
        });

        // Section 21: a player a mod owns, and the local player it cannot reach.
        tasks::schedule_after_ticks(26, move || {
            if !demo(21) {
                return;
            }
            let mut who = PlayerState::new("Tester")
                .with_uuid("0000-1111-2222")
                .with_game_mode(GameMode::Creative)
                .at(10.0, 70.0, -5.0);
            who.set_health(17.0);
            who.set_food_level(18);
            who.set_saturation(3.5);
            who.set_experience(0.5);
            who.set_level(7);
            who.set_score(42);
            who.set_locale("de_de");
            who.set_team(Some("red"));
            who.set_display_name("Tester (red)");
            who.set_skin("hello:skin");
            who.grant(PlayerPermission::Build);
            who.grant(PlayerPermission::Fly);
            who.apply_effect(Effect::new("minecraft:speed", 1, 1_000));
            who.unlock_advancement("minecraft:story/root");
            who.statistics.set("walk_one_cm", 10);
            who.statistics.add("walk_one_cm", 5);
            who.set_profile("xbox_id", "abc");
            who.inventory_mut()
                .set_slot(0, Some(ItemStack::new("minecraft:torch", 64)))
                .ok();
            who.inventory_mut()
                .set_slot(inventory::OFFHAND, Some(ItemStack::new("minecraft:shield", 1)))
                .ok();
            info!(
                "player: state={who} uuid={:?} health={}/{} food={} saturation={} exp={} level={} score={} locale={} team={:?} display={:?}",
                who.uuid,
                who.health,
                who.max_health,
                who.food_level,
                who.saturation,
                who.experience,
                who.level,
                who.score,
                who.locale,
                who.team,
                who.display_name,
            );
            info!(
                "player: distance={} alive={} full={} mode={:?} held={:?} armor={} offhand={:?} selected={}",
                who.distance_to((13.0, 74.0, -5.0)),
                who.is_alive(),
                who.is_full_health(),
                who.is_in_game_mode(GameMode::Creative),
                who.held_item().map(|stack| stack.to_string()),
                who.armor().into_iter().filter(Option::is_some).count(),
                who.offhand().map(|stack| stack.item().to_owned()),
                who.set_selected_slot(3),
            );
            info!(
                "player: effects={:?} active_now={} permissions={:?} has_fly={} advancements={:?} stats={:?} profile={:?}",
                who.effects.iter().map(|effect| (effect.name.clone(), effect.level())).collect::<Vec<_>>(),
                who.active_effects(500).len(),
                who.permissions.iter().map(|permission| permission.as_str()).collect::<Vec<_>>(),
                who.has_permission(PlayerPermission::Fly),
                who.advancements,
                who.statistics.values.keys().collect::<Vec<_>>(),
                who.profile.keys().collect::<Vec<_>>(),
            );
            who.reset_state();
            let revoked = who.revoke(PlayerPermission::Build);
            info!(
                "player: after reset effects={} flying={} sprinting={} revoked={}",
                who.effects.len(),
                who.flying,
                who.sprinting,
                revoked,
            );
            info!(
                "player: local: get={:?} all={} by_name={:?}",
                player::get_local(),
                player::known_only_to_the_client::get_all().len(),
                player::known_only_to_the_client::get_by_name("Tester"),
            );
            info!(
                "player: connection: latency={:?} ping={:?} state={:?} input={:?} camera={:?} skin={:?} cape={:?} profile={:?}",
                player::known_only_to_the_client::get_latency(),
                player::known_only_to_the_client::get_ping(),
                player::known_only_to_the_client::get_network_state(),
                player::known_only_to_the_client::get_input_state(),
                player::known_only_to_the_client::get_camera(),
                player::known_only_to_the_client::get_skin(),
                player::known_only_to_the_client::get_cape(),
                player::known_only_to_the_client::get_profile(),
            );
            info!(
                "player: say: message={} action_bar={} title={} sound={} particle={} open={} close={}",
                player::send_message("hello from the example"),
                player::send_action_bar("hello on the overlay"),
                player::send_title("big", "small"),
                player::play_sound("minecraft:click"),
                player::show_particle("minecraft:spark", 0.0, 64.0, 0.0),
                player::open_screen("inventory"),
                player::close_screen(),
            );
            info!(
                "player: authority: give={} take={} effect={} remove={} teleport={} mode={} locale={:?} refusals={:?}",
                player::give_item(&ItemStack::new("minecraft:stone", 1)),
                player::take_item("minecraft:stone", 1),
                player::apply_effect(Effect::new("minecraft:speed", 0, 100)),
                player::remove_effect("minecraft:speed"),
                player::teleport(0.0, 64.0, 0.0),
                player::set_game_mode(GameMode::Creative),
                player::get_locale(),
                player::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 22: effects a mod registers, and damage arithmetic it owns.
        tasks::schedule_after_ticks(28, move || {
            if !demo(22) {
                return;
            }
            effects::register_event_kinds();
            let mut who = PlayerState::new("Tester");
            info!(
                "effects: register={} type={:?}",
                effects::register(
                    EffectType::new("hello:haste", "Haste")
                        .beneficial()
                        .color(0x11_ff88)
                        .icon("hello:haste")
                        .particles("minecraft:spark")
                        .duration(3_000)
                        .amplifier(1)
                        .with_behavior()
                ),
                effects::get_by_id("hello:haste").map(|effect| effect.to_string()),
            );
            info!(
                "effects: apply={} has={} duration={:?} amplifier={:?} level={:?}",
                effects::apply(&mut who, "hello:haste", 0, 0),
                effects::has(&who, "hello:haste"),
                effects::get_duration(&who, "hello:haste"),
                effects::get_amplifier(&who, "hello:haste"),
                effects::get(&who, "hello:haste").map(|effect| effect.level()),
            );
            info!(
                "effects: set_duration={} set_amplifier={} now={:?}/{:?} icon={} particles={} behavior={} unregistered_apply={}",
                effects::set_duration(&mut who, "hello:haste", 9_000),
                effects::set_amplifier(&mut who, "hello:haste", 3),
                effects::get_duration(&who, "hello:haste"),
                effects::get_amplifier(&who, "hello:haste"),
                effects::register_icon("hello:haste", "hello:haste_icon"),
                effects::register_particles("hello:haste", "minecraft:spark"),
                effects::register_behavior("hello:haste"),
                effects::apply(&mut who, "an-unregistered-one", 100, 0),
            );
            info!(
                "effects: by_name={:?} all={} remove={} clear={} live_apply={} active={}",
                effects::get_by_name("Haste").map(|effect| effect.id),
                effects::get_all().len(),
                effects::remove(&mut who, "an-unregistered-one"),
                effects::clear(&mut who),
                effects::apply_live("hello:haste", 100, 0),
                who.effects.len(),
            );
            info!(
                "damage: register_type={} bypasses={:?} fire={:?}",
                damage::register_type(
                    damage::DamageType::new("hello:void")
                        .bypasses_armor()
                        .bypasses_resistance()
                        .fire()
                        .no_knockback()
                ),
                damage::get_type("hello:void").map(|kind| kind.bypasses_armor),
                damage::get_type("hello:void").map(|kind| kind.is_fire),
            );
            let plain = damage::DamageInstance::new(10.0, "minecraft:generic");
            let armored = damage::DamageInstance::new(10.0, "minecraft:generic")
                .with_armor(20.0)
                .with_toughness(2.0)
                .with_resistance(2)
                .with_protection(4);
            info!(
                "damage: plain={:.2} armored={:.2} reductions: armor={:.3} resistance={:.3} protection={:.3} cooldown={:.3}",
                damage::calculate(&plain),
                damage::calculate(&armored),
                damage::get_armor_reduction(&armored),
                damage::get_resistance(&armored),
                damage::get_enchantment_reduction(&armored),
                damage::get_cooldown(&armored),
            );
            let quick = damage::DamageInstance::new(10.0, "minecraft:generic").with_cooldown(100);
            let mut cancelled = plain.clone();
            damage::cancel(&mut cancelled);
            let mut knock = plain.clone();
            damage::set_knockback(&mut knock, 1.5);
            let through = damage::DamageInstance::new(10.0, "hello:void")
                .with_armor(20.0)
                .with_resistance(4);
            info!(
                "damage: cooldown_multiplier={:.3} quick={:.2} cancelled={:.2} knockback={:.1} void_through_armor={:.2} amount={:.2} knockback_default={:.1}",
                damage::get_cooldown(&quick),
                damage::calculate(&quick),
                damage::resolve(&cancelled),
                damage::get_knockback(&knock),
                damage::calculate(&through),
                damage::get_amount(&armored),
                damage::get_knockback(&plain),
            );
            info!(
                "damage: register_modifier={} modifier={:?} source={} by={:?} applied_to_player={:.1}",
                damage::register_modifier(damage::DamageModifier {
                    name: "hello:sharp".to_owned(),
                    kind: damage::DamageModifierKind::Multiply,
                    amount: 0.5,
                }),
                damage::get_modifier("hello:sharp").map(|modifier| modifier.amount),
                damage::register_source(
                    damage::DamageSource::new("hello:arrow")
                        .by("Tester")
                        .with_weapon("minecraft:bow")
                ),
                damage::get_source_by_id("hello:arrow").and_then(|source| source.attacker),
                damage::apply_to(&mut who, &plain),
            );
            info!(
                "damage: live_apply={} modify_calculation={} types={} events={} known={} refusals={:?}",
                damage::apply(&plain),
                damage::modify_calculation("hello:damage"),
                damage::get_types().len(),
                effects::event_kinds().len(),
                effects::has_event_kind("entity_damage"),
                effects::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "effects: unregister={} damage_unregister={}",
                effects::unregister("hello:haste"),
                damage::unregister_type("hello:void"),
            );
        });

        // Sections 23 and 24: physics a mod can compute, and a ray against a world it
        // supplies.
        tasks::schedule_after_ticks(30, move || {
            if !demo(1030) {
                return;
            }
            info!(
                "physics: constants={:?} friction(default)={} friction(ice)={}",
                physics::constants(),
                physics::get_friction(None),
                physics::get_friction(Some("minecraft:ice")),
            );
            let mut body = Body::new((0.5, 10.0, 0.5)).with_size(0.6, 1.8);
            body.on_ground = false;
            for _ in 0..40 {
                body.step(1.0);
            }
            info!(
                "physics: body={body} box={:?} eye={:?} fall={:.2} fall_damage={:.0}",
                body.collision_box(),
                body.eye_position(),
                body.fall_distance,
                body.fall_damage(),
            );
            let landed = body.land();
            body.apply_force(0.1, 0.0, 0.0);
            body.apply_impulse(0.0, 0.42, 0.0);
            body.apply_acceleration(0.0, 0.0, 0.1);
            body.set_collision_shape(0.6, 1.8);
            info!(
                "physics: landed={:.2} velocity={:?} ground={} fluid={} inside_block={} mass={:?}",
                landed,
                body.velocity,
                body.ground_state(),
                body.fluid_state(),
                body.is_inside_block(|_, y, _| y < 0),
                physics::get_mass(),
            );
            info!(
                "physics: live: velocity={:?} ground={:?} fluid={:?} fall={:?} box={:?} collide={:?} set={} noclip={} gravity={}",
                physics::known_only_to_the_client::get_velocity(),
                physics::known_only_to_the_client::get_ground_state(),
                physics::known_only_to_the_client::get_fluid_state(),
                physics::known_only_to_the_client::get_fall_distance(),
                physics::known_only_to_the_client::get_collision_box(),
                physics::known_only_to_the_client::is_colliding(),
                physics::set_velocity(1.0, 2.0, 3.0),
                physics::set_no_clip(true),
                physics::modify_gravity("hello:low"),
            );
            info!(
                "raycast: from_position={:?} hit={:?} face={:?} distance={:?} position={:?}",
                raycast::from_position((0.5, 0.5, 0.5)).ray(),
                raycast::get_hit_block(
                    &raycast::from_position((0.5, 0.5, 0.5))
                        .with_direction((1.0, 0.0, 0.0))
                        .with_range(10.0)
                        .cast(|x, _, _| x >= 3)
                ),
                raycast::get_hit_face(
                    &raycast::from_position((0.5, 0.5, 0.5))
                        .with_direction((1.0, 0.0, 0.0))
                        .with_range(10.0)
                        .cast(|x, _, _| x >= 3)
                ),
                raycast::get_hit_distance(
                    &raycast::from_position((0.5, 0.5, 0.5))
                        .with_direction((1.0, 0.0, 0.0))
                        .with_range(10.0)
                        .cast(|x, _, _| x >= 3)
                ),
                raycast::get_hit_position(
                    &raycast::from_position((0.5, 0.5, 0.5))
                        .with_direction((1.0, 0.0, 0.0))
                        .with_range(10.0)
                        .cast(|x, _, _| x >= 3)
                ),
            );
            let query = raycast::from_position((0.5, 0.5, 0.5))
                .with_direction((1.0, 0.0, 0.0))
                .with_range(10.0);
            let entities = vec![
                raycast::EntityBox::new("wisp", (2.5, 0.5, 0.5), (0.5, 0.5)),
                raycast::EntityBox::new("far", (9.5, 0.5, 0.5), (0.5, 0.5)),
            ];
            let result = raycast::get_result(&query, |x, _, _| x >= 3, &entities);
            info!(
                "raycast: result={result} clear={} entity={:?} blocks_only={:?} exclude={:?}",
                raycast::is_clear(&result),
                raycast::get_hit_entity(&result).map(|entity| entity.id),
                query.clone().blocks().cast_entities(&entities).is_none(),
                query.clone().exclude_entity("wisp").cast_entities(&entities).map(|entity| entity.id),
            );
            let camera = raycast::from_camera((0.5, 1.6, 0.5), 0.0, 0.0);
            let swept = query.clone().sweep(&body, 5.0, |x, _, _| x >= 3);
            info!(
                "raycast: camera_range={} camera_direction={:?} sweep={:?}",
                camera.ray().range,
                camera.ray().direction,
                swept.map(|hit| hit.block),
            );
            let target = raycast::interaction::get_target(&query, |x, _, _| x >= 3);
            info!(
                "interaction: target={target:?} is_some={} can_interact={} reach_creative={} reach_survival={} set_reach={}",
                target.is_some(),
                raycast::interaction::can_interact(&target, body.position, raycast::REACH_CREATIVE),
                raycast::interaction::get_reach_distance("creative"),
                raycast::interaction::get_reach_distance("survival"),
                raycast::interaction::set_reach_distance(10.0),
            );
            info!(
                "interaction: interact_block={} interact_entity={} attack={} use_item={} break={} place={} target={:?} handler={} cancel={} modify_reach={}",
                raycast::interaction::interact_block(3, 0, 0),
                raycast::interaction::interact_entity("wisp"),
                raycast::interaction::attack_entity("wisp"),
                raycast::interaction::use_item(),
                raycast::interaction::break_block(3, 0, 0),
                raycast::interaction::place_block(3, 0, 0, "minecraft:stone"),
                raycast::interaction::get_target(&query, |x, _, _| x >= 3).block.is_some(),
                raycast::interaction::register_handler("hello:use"),
                raycast::interaction::cancel_interaction(),
                raycast::interaction::modify_reach_check("hello:reach"),
            );
            info!(
                "physics/raycast: refusals={:?} {:?} interaction_refusals={:?}",
                physics::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                raycast::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                [
                    raycast::why_unknown("camera"),
                    raycast::why_unknown("world"),
                    raycast::why_unknown("entities"),
                    raycast::why_unknown("interaction"),
                    raycast::why_unknown("reach"),
                ],
            );
        });

        // Section 25: movement a mod can predict for itself, and the input the client owns.
        tasks::schedule_after_ticks(32, move || {
            if !demo(25) {
                return;
            }
            let profile = MovementProfile::new();
            let standing = movement::Input::default();
            let walking = movement::Input {
                forward: true,
                ..movement::Input::default()
            };
            let sprinting = movement::Input {
                sprint: true,
                ..walking
            };
            let sneaking = movement::Input {
                sneak: true,
                ..walking
            };
            let diagonal = movement::Input {
                forward: true,
                right: true,
                ..movement::Input::default()
            };
            info!(
                "movement: constants={:?} input(forward)={:?} input(diagonal)={:?} standing_moves={}",
                movement::constants(),
                walking.vector(),
                diagonal.vector(),
                standing.is_moving(),
            );
            info!(
                "movement: speeds walk={} sprint={} sneak={} standing={} jump_height={:.3} step={} can_step_0.7={}",
                profile.speed_for(&walking),
                profile.speed_for(&sprinting),
                profile.speed_for(&sneaking),
                profile.speed_for(&standing),
                profile.jump_height(),
                profile.can_step_up(0.6),
                profile.can_step_up(0.7),
            );
            info!(
                "movement: predict walk={:?} sprint={:?} jump={:?}",
                movement::predict(&profile, (0.0, 64.0, 0.0), &walking, 20),
                movement::predict(&profile, (0.0, 64.0, 0.0), &sprinting, 20),
                movement::predict(&profile, (0.0, 64.0, 0.0), &movement::Input {
                    jump: true,
                    ..walking
                }, 3),
            );
            let mut flying = profile.clone();
            flying.mode = movement::MovementMode::Flying;
            flying.flight_allowed = true;
            info!(
                "movement: flying mode={} up={:?} down={:?} swim_speed={} riding_speed={}",
                flying.mode,
                movement::predict(&flying, (0.0, 64.0, 0.0), &movement::Input {
                    jump: true,
                    ..movement::Input::default()
                }, 5),
                movement::predict(&flying, (0.0, 64.0, 0.0), &movement::Input {
                    sneak: true,
                    ..movement::Input::default()
                }, 5),
                movement::MovementMode::Swimming.speed(&profile),
                movement::MovementMode::Riding.speed(&profile),
            );
            let walked = movement::steps(&profile, (0.0, 64.0, 0.0), &walking, 4);
            info!(
                "movement: path={:?} first_step={:.3}",
                walked.iter().map(|step| step.to).collect::<Vec<_>>(),
                walked[0].distance(),
            );
            info!(
                "movement: live: input={:?} vector={:?} speed={:?} acceleration={:?} jump={:?} step={:?} fly={:?} sprint={:?} swim={:?} sneak={:?}",
                movement::get_input(),
                movement::get_input_vector(),
                movement::get_speed(),
                movement::get_acceleration(),
                movement::get_jump_strength(),
                movement::get_step_height(),
                movement::get_fly_speed(),
                movement::get_sprint_speed(),
                movement::get_swim_speed(),
                movement::get_sneak_speed(),
            );
            info!(
                "movement: server: prediction={:?} correction={:?} authoritative={:?} predicted={:?} set_prediction={}",
                movement::get_movement_prediction(),
                movement::get_server_correction(),
                movement::get_authoritative_position(),
                movement::get_predicted_position(),
                movement::set_movement_prediction(false),
            );
            info!(
                "movement: actions: jump={} sprint={}/{} sneak={}/{} fly={}/{} flight_allowed={} noclip={} mode={}",
                movement::jump(),
                movement::start_sprinting(),
                movement::stop_sprinting(),
                movement::start_sneaking(),
                movement::stop_sneaking(),
                movement::start_flying(),
                movement::stop_flying(),
                movement::set_flight_allowed(true),
                movement::set_noclip_allowed(true),
                movement::set_movement_mode(movement::MovementMode::Flying),
            );
            info!(
                "movement: set_speed={} set_jump={} hooks={}/{}/{} refusals={:?}",
                movement::set_speed(1.0),
                movement::set_jump_strength(1.0),
                movement::register_movement_modifier("hello:swift"),
                movement::register_input_handler("hello:keys"),
                movement::register_collision_modifier("hello:slick"),
                movement::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 26: world generation a mod ships, and noise it can run itself.
        tasks::schedule_after_ticks(34, move || {
            if !demo(26) {
                return;
            }
            info!(
                "worldgen: register_biome={} settings={:?}",
                worldgen::register_biome(
                    worldgen::Biome::new("hello:sky", "Sky")
                        .temperature(0.4)
                        .downfall(0.1)
                        .surface("minecraft:snow_block", "minecraft:dirt")
                        .feature("hello:clouds")
                ),
                worldgen::get_biome_settings("hello:sky").map(|biome| biome.to_string()),
            );
            info!(
                "worldgen: pieces: feature={} ore={} carver={} surface={} placement={} noise={} density={}",
                worldgen::register_feature(
                    worldgen::GenerationPiece::new(worldgen::PieceKind::Feature, "hello:trees")
                        .named("Trees")
                        .setting("count", "8")
                ),
                worldgen::register_ore(worldgen::GenerationPiece::new(worldgen::PieceKind::Ore, "hello:ore")),
                worldgen::register_carver(worldgen::GenerationPiece::new(worldgen::PieceKind::Carver, "hello:cave")),
                worldgen::register_surface_rule(worldgen::GenerationPiece::new(worldgen::PieceKind::SurfaceRule, "hello:rule")),
                worldgen::register_placement_rule(worldgen::GenerationPiece::new(worldgen::PieceKind::PlacementRule, "hello:place")),
                worldgen::register_noise(worldgen::GenerationPiece::new(worldgen::PieceKind::Noise, "hello:noise")),
                worldgen::register_density_function(worldgen::GenerationPiece::new(worldgen::PieceKind::DensityFunction, "hello:density")),
            );
            info!(
                "worldgen: dimension={} type={} source={} generator={} structure={} callback={} features={} pieces={}",
                worldgen::register_dimension(worldgen::GenerationPiece::new(worldgen::PieceKind::Dimension, "hello:sky_dim")),
                worldgen::register_dimension_type(worldgen::GenerationPiece::new(worldgen::PieceKind::DimensionType, "hello:sky_type")),
                worldgen::register_biome_source(worldgen::GenerationPiece::new(worldgen::PieceKind::BiomeSource, "hello:source")),
                worldgen::register_chunk_generator(worldgen::GenerationPiece::new(worldgen::PieceKind::ChunkGenerator, "hello:gen")),
                worldgen::register_structure(worldgen::GenerationPiece::new(worldgen::PieceKind::Structure, "hello:tower")),
                worldgen::register_chunk_callback("hello:gen"),
                worldgen::add_placed_feature("hello:sky", "hello:trees"),
                worldgen::get_pieces(None).len(),
            );
            info!(
                "worldgen: settings: noise={:?} generation={:?} structure={:?} biomes={} by_name={:?}",
                worldgen::get_noise_settings("hello:noise").map(|piece| piece.name),
                worldgen::get_generation_settings("hello:gen").map(|piece| piece.to_string()),
                worldgen::get_structure_settings("hello:tower").is_some(),
                worldgen::get_biomes().len(),
                worldgen::get_biome_by_name("Sky").map(|biome| biome.id),
            );
            let mut random = worldgen::Random::new(42);
            let mut other = worldgen::Random::new(42);
            info!(
                "worldgen: random: same_seed_repeats={} draw={} range={} chance={} pick={:?} chunk_seeded={}",
                random.next_u32() == other.next_u32(),
                worldgen::Random::new(7).next_f64(),
                worldgen::Random::new(7).next_range(1, 6),
                worldgen::Random::new(7).chance(0.5),
                worldgen::Random::new(7).pick(&["a", "b", "c"]),
                worldgen::Random::for_position(9, 1, 2).next_u32()
                    == worldgen::Random::for_position(9, 1, 2).next_u32(),
            );
            let noise = worldgen::Noise::new(42);
            info!(
                "worldgen: noise: at={:.4} fractal={:.4} two_seeds_differ={} same_seed_same={}",
                noise.at_2d(1.5, 2.5),
                noise.fractal(0.5, 0.5, 4, 0.5),
                noise.at_2d(1.5, 2.5) != worldgen::Noise::new(43).at_2d(1.5, 2.5),
                noise.at_2d(1.5, 2.5) == worldgen::Noise::new(42).at_2d(1.5, 2.5),
            );
            let context = worldgen::GenerationContext::new(42, (0, 0)).dimension("hello:sky_dim");
            let generator = |_x: usize, _z: usize, y: i32| -> String {
                if y < 64 {
                    "minecraft:stone".to_owned()
                } else {
                    "minecraft:air".to_owned()
                }
            };
            let chunk = worldgen::generate_chunk(&context, &generator);
            let again = worldgen::generate_chunk(&context, &generator);
            info!(
                "worldgen: chunk={chunk} origin={:?} height={:?} biome={:?} same_twice={} solid={}",
                context.origin(),
                worldgen::get_height_at(&chunk, 0, 0),
                worldgen::get_biome_at(&chunk, 0, 0),
                chunk.blocks == again.blocks,
                chunk.solid_count(),
            );
            let mut editable = worldgen::Chunk::empty((0, 0));
            editable.set_block(0, 64, 0, "minecraft:stone");
            let populated = worldgen::populate_chunk(&mut editable, &context, &generator);
            info!(
                "worldgen: populate={} feature={} structure={} block={:?}",
                populated,
                worldgen::place_feature(&mut editable, 1, 70, 1, "minecraft:oak_log"),
                worldgen::place_structure(&mut editable, 2, 70, 2, (2, 2, 2), "minecraft:oak_planks"),
                editable.block(0, 65, 0),
            );
            info!(
                "worldgen: live: seed={:?} random={:?} generator={:?} context={:?} order={} refusals={:?}",
                worldgen::get_seed(),
                worldgen::get_random(),
                worldgen::get_generator(),
                worldgen::create_generation_context(),
                worldgen::modify_generation_order("hello:first"),
                worldgen::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "worldgen: unregister: biome={} feature={} structure={}",
                worldgen::unregister_biome("hello:sky"),
                worldgen::unregister_feature("hello:trees"),
                worldgen::unregister_structure("hello:tower"),
            );
        });

        // Section 27: recipes a mod ships, and the matching it can run itself.
        tasks::schedule_after_ticks(36, move || {
            if !demo(27) {
                return;
            }
            info!(
                "recipes: register_shaped={} recipe={:?}",
                recipes::register_shaped(recipes::pattern(
                    recipes::shaped("hello:torches", "minecraft:torch", 4),
                    &["C", "S"],
                )
                .key('C', "minecraft:coal")
                .key('S', "minecraft:stick")
                .category("misc")
                .experience(0.1)),
                recipes::get_by_id("hello:torches").map(|recipe| recipe.to_string()),
            );
            info!(
                "recipes: register_shapeless={} smelting={} blasting={} brewing={} smithing={} machine={}",
                recipes::register_shapeless(recipes::from(
                    recipes::shapeless("hello:planks", "minecraft:oak_planks", 4),
                    &["minecraft:oak_log"],
                )),
                recipes::register_smelting_recipe(
                    Recipe::new(recipes::RecipeType::Smelting, "hello:glass", ItemStack::new("minecraft:glass", 1))
                        .ingredient(recipes::IngredientStack::of("minecraft:sand", 1))
                        .experience(0.1)
                ),
                recipes::register_blasting_recipe(
                    Recipe::new(recipes::RecipeType::Blasting, "hello:iron", ItemStack::new("minecraft:iron_ingot", 1))
                        .ingredient(recipes::IngredientStack::of("minecraft:iron_ore", 1))
                ),
                recipes::register_brewing_recipe(
                    Recipe::new(recipes::RecipeType::Brewing, "hello:potion", ItemStack::new("minecraft:potion", 1))
                        .ingredient(recipes::IngredientStack::of("minecraft:water_bottle", 1))
                ),
                recipes::register_smithing_recipe(
                    Recipe::new(recipes::RecipeType::Smithing, "hello:sword", ItemStack::new("minecraft:diamond_sword", 1))
                        .ingredient(recipes::IngredientStack::of("minecraft:diamond_sword", 1))
                ),
                recipes::register_machine_recipe(
                    Recipe::new(recipes::RecipeType::Machine, "hello:powder", ItemStack::new("minecraft:gunpowder", 2))
                        .ingredient(recipes::IngredientStack::of("minecraft:flint", 1))
                        .time(60)
                ),
            );
            let coal = ItemStack::new("minecraft:coal", 1);
            let stick = ItemStack::new("minecraft:stick", 1);
            let column = vec![Some(coal.clone()), Some(stick.clone())];
            let mirrored = vec![Some(stick.clone()), Some(coal.clone())];
            let three = recipes::grid_from(
                &[ItemStack::new("minecraft:coal", 1), ItemStack::new("minecraft:stick", 1)],
                3,
            );
            info!(
                "recipes: match: column={} mirrored={} three_by_three={} wrong_item={} partial={}",
                recipes::can_craft(&column, 1),
                recipes::can_craft(&mirrored, 1),
                recipes::can_craft(&three, 3),
                recipes::can_craft(&[Some(ItemStack::new("minecraft:dirt", 1)), Some(stick.clone())], 1),
                recipes::can_craft(&[Some(coal.clone())], 1),
            );
            info!(
                "recipes: craft={:?} all={:?} found={} ingredients={:?} result={:?} category={:?} time={:?} experience={:?}",
                recipes::craft(&column, 1).map(|stack| stack.to_string()),
                recipes::craft_all(&column, 1)
                    .into_iter()
                    .map(|stack| stack.to_string())
                    .collect::<Vec<_>>(),
                recipes::find_matching(&column, 1).len(),
                recipes::get_ingredients("hello:torches")
                    .into_iter()
                    .map(|ingredient| ingredient.label())
                    .collect::<Vec<_>>(),
                recipes::get_result("hello:torches").map(|stack| stack.count()),
                recipes::get_category("hello:torches"),
                recipes::get_cook_time("hello:glass"),
                recipes::get_experience("hello:glass"),
            );
            info!(
                "recipes: fuels: coal={:?} stick={:?} unknown={:?} register={} batch={} types={} recipe_types={}",
                recipes::get_fuel_time("minecraft:coal"),
                recipes::get_fuel_time("minecraft:stick"),
                recipes::get_fuel_time("minecraft:dirt"),
                recipes::register_fuel("hello:peat", 400),
                recipes::batch_size("hello:glass", "minecraft:coal"),
                recipes::get_all().len(),
                recipes::get(Some(recipes::RecipeType::Smelting)).len(),
            );
            info!(
                "recipes: register_type={} names={} serializer={} serializers={} condition={} conditions={}",
                recipes::register_type("hello:alchemy"),
                recipes::get_type_names().len(),
                recipes::register_serializer("hello:alchemy"),
                recipes::get_serializers().len(),
                recipes::register_condition("hello:loaded"),
                recipes::get_conditions().len(),
            );
            info!(
                "recipes: by_id={:?} for_result={} type={:?} replace={} remove={} remaining={}",
                recipes::get_by_id("hello:torches").map(|recipe| recipe.id),
                recipes::get_for_result("minecraft:torch").len(),
                recipes::get_type("hello:torches"),
                recipes::replace(recipes::shaped("hello:torches", "minecraft:torch", 4)),
                recipes::remove("hello:potion"),
                recipes::get_remaining_items("hello:torches").len(),
            );
            info!(
                "recipes: client: recipes={} serializer={:?} craft_into_player={} refusals={:?}",
                recipes::get_client_recipes().len(),
                recipes::get_serializer("minecraft:crafting_shaped"),
                recipes::craft_into_player(&column, 1),
                recipes::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 28: one registry engine for everything a mod ships.
        tasks::schedule_after_ticks(38, move || {
            if !demo(28) {
                return;
            }
            let gems = registry::register_custom_registry("hello:gems");
            info!(
                "registry: custom={:?} registries={} register={:?} keyed={:?}",
                gems.name,
                registry::registered().len(),
                registry::register_in("hello:gems", "hello:ruby", ConfigValue::Integer(3)),
                registry::register_keyed("hello:gem", "gem", ConfigValue::Integer(7)),
            );
            info!(
                "registry: get={:?} by_key={:?} id={:?} key={:?} contains={} size={} version={}",
                registry::get("hello:gem"),
                registry::get_by_key("gem"),
                registry::get_id("gem"),
                registry::get_key("hello:gem"),
                registry::contains("hello:gem"),
                registry::get_size(),
                registry::get_version(),
            );
            info!(
                "registry: in_gems={:?} batch={:?} replace={:?} values={} keys={} entries={}",
                registry::get_in("hello:gems", "hello:ruby"),
                registry::register_batch(&[
                    ("hello:a", ConfigValue::Integer(1)),
                    ("hello:b", ConfigValue::Integer(2)),
                ]),
                registry::replace("hello:a", ConfigValue::Integer(9)),
                registry::values().len(),
                registry::keys().len(),
                registry::entries().len(),
            );
            info!(
                "registry: iterate={:?} changes={:?}",
                registry::iterate(|id, value| format!("{id}={}", value.render())),
                registry::get_changes()
                    .into_iter()
                    .map(|change| change.to_string())
                    .collect::<Vec<_>>(),
            );
            let snapshot = registry::snapshot();
            info!(
                "registry: freeze={} frozen={} blocked={:?} unfreeze={} restore={:?} snapshot={}",
                registry::freeze(None),
                registry::is_frozen(None),
                registry::register("hello:blocked", ConfigValue::Integer(1)),
                registry::unfreeze(None),
                snapshot.as_ref().map(registry::restore),
                snapshot.as_ref().map(|snapshot| snapshot.size()).unwrap_or_default(),
            );
            info!(
                "registry: validate={:?} serializer={} codec={} serializers={} manager={:?} client_registries={}",
                registry::validate(),
                registry::register_serializer("hello:gem", "key=value"),
                registry::register_codec("hello:other", "key=value"),
                registry::serializers().len(),
                registry::get_registry_manager(),
                registry::CLIENT_REGISTRIES.len(),
            );
            let handle = registry::register_custom_registry("Blocks");
            info!(
                "registry: own_blocks={:?} register={:?} get={:?} unregister={:?}",
                handle.name,
                registry::register_in("Blocks", "hello:lantern", ConfigValue::Integer(1)),
                registry::get_in("Blocks", "hello:lantern"),
                registry::unregister_in("Blocks", "hello:lantern"),
            );
        });

        // Section 29: the render lifecycle this host can hook, and the renderer it cannot.
        tasks::schedule_after_ticks(40, move || {
            if !demo(29) {
                return;
            }
            render::register_event_kinds();
            let frames = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
            let counter = std::sync::Arc::clone(&frames);
            let start = render::on_frame_start(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            });
            let hud = render::on_hud_render(|_| {});
            let end = render::on_frame_end(|_| {});
            let reload = render::on_resource_reload(|_| {});
            info!(
                "render: subscriptions start={start} hud={hud} end={end} reload={reload} listeners={}",
                render::listener_count(cinnaloader::events::kind::RENDER_TICK),
            );
            info!(
                "render: register={} transparent={} pass={} layer={}",
                render::register_renderer(
                    render::Renderer::new("hello:outline")
                        .layer("hello:lines")
                        .order(10)
                ),
                render::register_renderer(
                    render::Renderer::new("hello:glow")
                        .layer("hello:glow")
                        .order(5)
                        .transparent()
                        .no_cull()
                ),
                render::register_pass("hello:post"),
                render::register_render_layer("hello:solid"),
            );
            info!(
                "render: declared={:?} outline={:?}",
                render::get_renderers()
                    .into_iter()
                    .map(|renderer| renderer.to_string())
                    .collect::<Vec<_>>(),
                render::get_renderer("hello:outline").map(|renderer| renderer.layer),
            );
            info!(
                "render: cache={} resources={} render_thread={} request_frame={} events={} known={}",
                render::invalidate_cache(),
                render::reload_resources(),
                render::is_render_thread(),
                render::request_frame(),
                render::event_kinds().len(),
                render::has_event_kind("render_chunk"),
            );
            info!(
                "render: client-side: context={:?} state={:?} camera={:?} view={:?} projection={:?} model={:?}",
                render::get_context(),
                render::get_state(),
                render::get_camera(),
                render::get_view_matrix(),
                render::get_projection_matrix(),
                render::get_model_matrix(),
            );
            info!(
                "render: client-side: framebuffer={:?} target={:?} device={:?} backend={:?} passes: world={} entity={} block={} chunk={} hand={}",
                render::get_framebuffer(),
                render::get_render_target(),
                render::get_device(),
                render::get_backend(),
                render::on_world_render_start(|_| {}),
                render::on_entity_render(|_| {}),
                render::on_block_render(|_| {}),
                render::on_chunk_render(|_| {}),
                render::on_hand_render(|_| {}),
            );
            info!(
                "render: client-side: screen={} particle={} nameplate={} outline={} shadow={} post={} texture={} shader={} refusals={:?}",
                render::on_screen_render(|_| {}),
                render::on_particle_render(|_| {}),
                render::on_nameplate_render(|_| {}),
                render::on_outline_render(|_| {}),
                render::on_shadow_render(|_| {}),
                render::on_post_process(|_| {}),
                render::on_texture_upload(|_| {}),
                render::on_shader_reload(|_| {}),
                render::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "render: unsubscribe={} {} {} {} unregister={} pass={} layer={}",
                render::unsubscribe(start),
                render::unsubscribe(hud),
                render::unsubscribe(end),
                render::unsubscribe(reload),
                render::unregister_renderer("hello:glow"),
                render::unregister_pass("hello:post"),
                render::unregister_render_layer("hello:solid"),
            );
        });

        // Section 30: a draw list a mod builds, and the little of it this host can draw.
        tasks::schedule_after_ticks(42, move || {
            if !demo(30) {
                return;
            }
            let mut list = draw::DrawList::new();
            list.set_color([1.0, 0.2, 0.2, 1.0]).set_alpha(0.9);
            list.translate(10.0, 20.0);
            list.push_transform();
            list.scale(2.0, 2.0).rotate(90.0);
            list.line(0.0, 0.0, 10.0, 0.0);
            list.pop_transform();
            list.rect(0.0, 0.0, 100.0, 20.0);
            list.filled_rect(0.0, 0.0, 100.0, 20.0);
            list.rounded_rect(0.0, 0.0, 60.0, 20.0, 4.0);
            list.gradient_rect(0.0, 0.0, 60.0, 20.0, [1.0; 4], [0.0, 0.0, 0.0, 1.0]);
            list.circle(30.0, 10.0, 8.0);
            list.filled_circle(30.0, 10.0, 8.0);
            list.ellipse(30.0, 10.0, 8.0, 4.0);
            list.triangle((0.0, 0.0), (10.0, 0.0), (5.0, 8.0));
            list.polygon(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);
            list.polyline(&[(0.0, 0.0), (5.0, 5.0), (10.0, 0.0)]);
            list.quad([(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
            list.wireframe_box((0.0, 0.0, 0.0), (1.0, 1.0, 1.0));
            list.sphere((0.0, 64.0, 0.0), 1.0, 16);
            list.cylinder((0.0, 64.0, 0.0), 1.0, 2.0);
            list.cone((0.0, 64.0, 0.0), 1.0, 2.0);
            list.capsule((0.0, 64.0, 0.0), (0.0, 65.0, 0.0), 0.5);
            list.mesh("hello:mesh", 24);
            list.text("hello from a draw list", 4.0, 4.0);
            list.text_shadow("with a shadow", 4.0, 20.0);
            list.outlined_text("outlined", 4.0, 36.0);
            list.textured_rect("hello:tex", 0.0, 0.0, 8.0, 8.0);
            list.sprite("hello:sprite", 0.0, 0.0);
            list.icon(draw::IconKind::Item, "minecraft:stone", 0.0, 0.0);
            list.scissor(0.0, 0.0, 100.0, 20.0);
            info!(
                "draw: list={list} commands={} transform={:?} state={:?}",
                list.len(),
                list.transform().apply((1.0, 1.0)),
                list.state().color,
            );
            let (drawn, undrawn) = list.flush();
            info!(
                "draw: flushed drawn={drawn} undrawn={undrawn} total={:?} empty={}",
                draw::get_draw_calls(),
                list.is_empty(),
            );
            info!(
                "draw: transform: translated={:?} rotated={:?} scaled={:?} inverted={:?}",
                draw::Transform::translation(5.0, 5.0).apply((1.0, 1.0)),
                draw::Transform::rotation(90.0).apply((1.0, 0.0)),
                draw::Transform::scaling(3.0, 3.0).apply((1.0, 1.0)),
                draw::Transform::scaling(2.0, 2.0).inverted().map(|i| i.apply((4.0, 4.0))),
            );
            info!(
                "draw: blit={} refusals={:?}",
                draw::overlay("a direct overlay line"),
                draw::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 31: a GPU ledger a mod keeps, and the device it cannot touch.
        tasks::schedule_after_ticks(44, move || {
            if !demo(31) {
                return;
            }
            let vertex = graphics::create_vertex_buffer("hello:vertices", 4096, "vertex");
            let uniform = graphics::create_uniform_buffer("hello:uniforms", 256, "uniform");
            let texture = graphics::create_texture("hello:atlas", "rgba8", 32, 32);
            let _shader = graphics::create_shader("hello:shader", 2048);
            let _pipeline = graphics::create_render_pipeline("hello:pipeline");
            let _target = graphics::create_render_target("hello:target", 64, 64);
            let _depth = graphics::create_depth_buffer("hello:depth", 64, 64);
            info!(
                "graphics: ledger count={} bytes={} resources={:?}",
                graphics::get_resource_count(),
                graphics::get_memory_usage(),
                graphics::get_resources()
                    .into_iter()
                    .map(|resource| format!("{} {}B", resource.kind, resource.size))
                    .collect::<Vec<_>>(),
            );
            info!(
                "graphics: vertex={vertex} on_gpu={} texture={:?} released={} count_now={}",
                graphics::is_created_on_the_gpu(vertex),
                graphics::get_resource(texture).map(|resource| resource.to_string()),
                graphics::release_resource(vertex),
                graphics::get_resource_count(),
            );
            graphics::note_validation_error("a render target with no depth buffer");
            graphics::note_device_lost("the example said so");
            info!(
                "graphics: validation={:?} device_lost={:?} backends={:?}",
                graphics::get_validation_errors(),
                graphics::get_device_lost_reason(),
                graphics::BACKENDS,
            );
            info!(
                "graphics: device={:?} adapter={:?} backend={:?} caps={} limits={} submit={}",
                graphics::get_device(),
                graphics::get_adapter(),
                graphics::get_backend(),
                graphics::get_capabilities().len(),
                graphics::get_limits().len(),
                graphics::submit(),
            );
            info!(
                "graphics: read_buffer={:?} write_buffer={} read_texture={:?} write_texture={} resize={} {} refusals={:?}",
                graphics::read_buffer(uniform).map(|bytes| bytes.len()),
                graphics::write_buffer(uniform, &[1, 2, 3]),
                graphics::read_texture(texture).map(|bytes| bytes.len()),
                graphics::write_texture(texture, &[1, 2, 3]),
                graphics::resize_texture(texture, 64, 64),
                graphics::resize_render_target(_target, 128, 128),
                graphics::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 32: shaders a mod ships, and the pipeline it cannot reach.
        tasks::schedule_after_ticks(46, move || {
            if !demo(32) {
                return;
            }
            shaders::register_include("hello:common", "uniform mat4 u_view;\nuniform float u_time;");
            info!(
                "shaders: register={} include={}",
                shaders::register(
                    shaders::Shader::new("hello:outline", shaders::ShaderStage::Fragment)
                        .source(
                            "uniform mat4 u_proj;\nuniform vec4 u_color;\nin vec2 v_uv;\nvoid main() {}"
                        )
                        .include("hello:common")
                        .define("OUTLINE_WIDTH", "2")
                ),
                shaders::register_include("hello:extra", "uniform vec3 u_glow;"),
            );
            info!(
                "shaders: shader={:?} uniforms={:?} attributes={:?}",
                shaders::get("hello:outline").map(|shader| shader.to_string()),
                shaders::get_uniforms("hello:outline")
                    .into_iter()
                    .map(|uniform| format!("{} {}", uniform.kind, uniform.name))
                    .collect::<Vec<_>>(),
                shaders::get_attributes("hello:outline")
                    .into_iter()
                    .map(|attribute| attribute.name)
                    .collect::<Vec<_>>(),
            );
            info!(
                "shaders: uniform(u_color)={:?} proj_components={:?} reflection={:?}",
                shaders::get_uniform("hello:outline", "u_color").map(|uniform| uniform.components()),
                shaders::get_uniform("hello:outline", "u_proj").map(|uniform| uniform.components()),
                shaders::get_reflection("hello:outline"),
            );
            info!(
                "shaders: define={} variant={} uniform_value={} read_back={:?}",
                shaders::set_define("hello:outline", "ALPHA_TEST", "1"),
                shaders::set_variant("hello:outline", "alpha_test"),
                shaders::set_uniform("hello:outline", "u_color", "1,0,0,1"),
                shaders::get_uniform_value("hello:outline", "u_color"),
            );
            info!(
                "shaders: material={} post_process={} pass={}",
                shaders::register_material(shaders::Material {
                    id: "hello:glow".to_owned(),
                    owner: "hello_mod".to_owned(),
                    shader: "hello:outline".to_owned(),
                    defines: Default::default(),
                }),
                shaders::register_post_process(shaders::PostProcess {
                    id: "hello:bloom".to_owned(),
                    owner: "hello_mod".to_owned(),
                    shader: "hello:outline".to_owned(),
                    hook: shaders::ShaderHook::BeforeFinalComposite,
                    order: 10,
                    active: false,
                }),
                shaders::register_post_process(shaders::PostProcess {
                    id: "hello:blur".to_owned(),
                    owner: "hello_mod".to_owned(),
                    shader: "hello:outline".to_owned(),
                    hook: shaders::ShaderHook::BeforeWorldOpaque,
                    order: 0,
                    active: false,
                }),
            );
            info!(
                "shaders: passes={:?} for_hook={} apply={} active={:?} remove={} active_now={:?}",
                shaders::get_post_processes()
                    .into_iter()
                    .map(|pass| pass.id)
                    .collect::<Vec<_>>(),
                shaders::get_post_processes_for(shaders::ShaderHook::BeforeFinalComposite).len(),
                shaders::apply_post_process("hello:bloom"),
                shaders::get_active_shaders(),
                shaders::remove_post_process("hello:bloom"),
                shaders::get_active_shaders(),
            );
            info!(
                "shaders: hooks={} audit={:?}",
                shaders::ShaderHook::all().len(),
                shaders::ShaderHook::all()
                    .into_iter()
                    .map(|hook| hook.as_str())
                    .collect::<Vec<_>>(),
            );
            info!(
                "shaders: client-side: load={} compile={} variant={} all={} reload={} errors={:?} features={:?} cache={:?} clear={}",
                shaders::load("hello:outline"),
                shaders::compile("hello:outline"),
                shaders::compile_variant("hello:outline", "alpha_test"),
                shaders::compile_all(),
                shaders::reload(),
                shaders::get_compile_errors(),
                shaders::get_supported_features(),
                shaders::get_pipeline_cache(),
                shaders::clear_pipeline_cache(),
            );
            let pipeline = shaders::create_pipeline("hello:pipeline");
            info!(
                "shaders: pipeline_handle={pipeline} kind={:?} on_gpu={} refusals={:?} material={:?}",
                graphics::get_resource(pipeline).map(|resource| resource.kind),
                graphics::is_created_on_the_gpu(pipeline),
                shaders::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                shaders::get_material("hello:glow").map(|material| material.shader),
            );
        });

        // Section 33: textures and sprites a mod owns, and the upload it cannot do.
        tasks::schedule_after_ticks(48, move || {
            if !demo(33) {
                return;
            }
            info!(
                "textures: load={} format={:?} dimensions={:?} pixels={:?} memory={}",
                textures::load_from_bytes("hello:square", &[255u8; 64], 4, 4, "rgba8"),
                textures::get_format("hello:square"),
                textures::get_dimensions("hello:square"),
                textures::get_pixels("hello:square").map(|pixels| pixels.len()),
                textures::get_memory_usage(),
            );
            info!(
                "textures: pixel={:?} set={} now={:?} region={} id={:?}",
                textures::get_pixel("hello:square", 0, 0),
                textures::set_pixel("hello:square", 1, 1, [10, 20, 30, 255]),
                textures::get_pixel("hello:square", 1, 1),
                textures::update_region("hello:square", 2, 2, 2, 1, &[9, 9, 9, 9, 8, 8, 8, 8]),
                textures::get_id("hello:square"),
            );
            info!(
                "textures: atlas={} sprite_a={} sprite_b={} sprite_c={}",
                textures::register_atlas("hello:atlas", 16, 16, "rgba8"),
                textures::register_sprite("hello:atlas", "hello:a", 8, 8),
                textures::register_sprite("hello:atlas", "hello:b", 8, 8),
                textures::register_sprite("hello:atlas", "hello:c", 8, 8),
            );
            info!(
                "textures: sprites={:?} uv_a={:?} uv_c={:?} stitched={} texture={:?}",
                textures::get_sprites("hello:atlas")
                    .into_iter()
                    .map(|sprite| format!("{}@{},{}", sprite.id, sprite.x, sprite.y))
                    .collect::<Vec<_>>(),
                textures::get_uv("hello:a"),
                textures::get_uv("hello:c"),
                textures::stitch_atlas("hello:atlas"),
                textures::get("hello:atlas").map(|texture| texture.to_string()),
            );
            textures::register_animation("hello:square", &[(0, 100), (1, 200), (2, 100)]);
            info!(
                "textures: animation frames={:?} at150={:?} at400={:?} set_frame={}",
                textures::get("hello:square").map(|texture| texture.frames.len()),
                textures::frame_at("hello:square", 150),
                textures::frame_at("hello:square", 400),
                textures::set_animation_frame("hello:square", 2),
            );
            info!(
                "textures: settings filter={} wrap={} anisotropy={} mipmap={}",
                textures::set_filter("hello:square", textures::Filter::Linear),
                textures::set_wrap_mode("hello:square", textures::WrapMode::Repeat),
                textures::set_anisotropy("hello:square", 4),
                textures::generate_mipmap("hello:square"),
            );
            info!(
                "textures: client-side: load={} bind={} unbind={} delete={} reload={} refusals={:?} raw={}",
                textures::load("hello:square"),
                textures::bind("hello:square"),
                textures::unbind(),
                textures::delete("hello:square"),
                textures::reload("hello:square"),
                textures::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                textures::is_raw_image("art/thing.png"),
            );
        });

        // Section 34: models a mod ships, and an animation it drives itself.
        tasks::schedule_after_ticks(50, move || {
            if !demo(34) {
                return;
            }
            info!(
                "models: register={} model={:?}",
                models::register(
                    Model::new("hello:cube")
                        .bone(Bone::new("body").at(0.0, 1.0, 0.0))
                        .bone(Bone::new("lid").parent("body").at(0.0, 2.0, 0.0))
                        .part(
                            ModelPart::new("shell", "body")
                                .vertices(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0])
                                .indices(&[0, 1, 2])
                                .material("minecraft:entity")
                                .texture("hello:skin")
                        )
                        .part(ModelPart::new("top", "lid"))
                ),
                models::get("hello:cube").map(|model| model.to_string()),
            );
            info!(
                "models: bones={:?} parts={:?} materials={:?} bounds={:?} chain={:?}",
                models::get_bones("hello:cube")
                    .into_iter()
                    .map(|bone| (bone.name, bone.parent))
                    .collect::<Vec<_>>(),
                models::get_parts("hello:cube")
                    .into_iter()
                    .map(|part| part.name)
                    .collect::<Vec<_>>(),
                models::get_materials("hello:cube"),
                models::get_bounds("hello:cube"),
                models::get("hello:cube").map(|model| model.chain_of("lid")),
            );
            info!(
                "models: vertices={:?} indices={:?} set_material={} set_texture={} transform={} visibility={}",
                models::get_vertices("hello:cube")
                    .into_iter()
                    .map(|(part, vertices)| (part, vertices.len()))
                    .collect::<Vec<_>>(),
                models::get_indices("hello:cube")
                    .into_iter()
                    .map(|(part, indices)| (part, indices.len()))
                    .collect::<Vec<_>>(),
                models::set_material("hello:cube", "shell", "minecraft:glow"),
                models::set_texture("hello:cube", "shell", "hello:other_skin"),
                models::set_transform("hello:cube", "lid", models::BoneTransform::rotated(45.0, 0.0, 0.0)),
                models::set_visibility("hello:cube", "top", false),
            );
            let exported = models::export("hello:cube").unwrap_or_default();
            info!(
                "models: export={:?} import={:?} loader={} entity_renderer={} block_renderer={} item_renderer={}",
                exported.lines().count(),
                models::import(&exported).map(|model| model.id),
                models::register_loader("hello:own_format"),
                models::register_entity_renderer("hello:cube_entity"),
                models::register_block_renderer("minecraft:stone"),
                models::register_item_renderer("minecraft:stick"),
            );
            info!(
                "models: renderers={:?} loaders={:?} client: load={} reload={} refusals={:?}",
                models::get_renderers().len(),
                models::get_loaders().len(),
                models::load("hello:cube"),
                models::reload(),
                models::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            models::register_animation(
                Animation::new("hello:open")
                    .frame(Keyframe::new("lid", 0, models::BoneTransform::at(0.0, 2.0, 0.0)))
                    .frame(Keyframe::new("lid", 1000, models::BoneTransform::rotated(90.0, 0.0, 0.0)))
                    .looping(models::LoopMode::Once),
            );
            models::register_animation(
                Animation::new("hello:idle")
                    .frame(Keyframe::new("body", 0, models::BoneTransform::at(0.0, 1.0, 0.0)))
                    .frame(Keyframe::new("body", 500, models::BoneTransform::at(0.0, 1.1, 0.0))),
            );
            info!(
                "animations: animations={:?} bones={:?} length={:?}",
                models::get_animations()
                    .into_iter()
                    .map(|animation| format!("{animation}"))
                    .collect::<Vec<_>>(),
                models::get_animation("hello:open").map(|animation| animation.bones()),
                models::get_animation("hello:open").map(|animation| animation.length()),
            );
            info!(
                "animations: play={} time={:?} advance={:?} speed={} advance2={:?} pause={} advance3={:?} resume={}",
                models::play("hello:open"),
                models::get_time("hello:open"),
                {
                    models::advance(250);
                    models::get_time("hello:open")
                },
                models::set_speed("hello:open", 2.0),
                {
                    models::advance(250);
                    models::get_time("hello:open")
                },
                models::pause("hello:open"),
                {
                    models::advance(250);
                    models::get_time("hello:open")
                },
                models::resume("hello:open"),
            );
            info!(
                "animations: crossfade={} state={:?} blend={:?}",
                models::crossfade("hello:open", "hello:idle", 1000),
                models::get_state()
                    .into_iter()
                    .map(|playback| (playback.animation, playback.weight))
                    .collect::<Vec<_>>(),
                {
                    models::advance(500);
                    models::blend()
                },
            );
            info!(
                "animations: skeleton={:?} bone={:?} set_bone={} set_time={} parameter={:?}",
                models::get_skeleton("hello:cube", "hello:idle", 250).len(),
                models::get_bone_transform("hello:cube", "hello:idle", "body", 250),
                models::set_bone_transform("hello:cube", "body", models::BoneTransform::at(0.0, 0.0, 0.0)),
                models::set_time("hello:idle", 100),
                {
                    models::set_parameter("open", 0.5);
                    models::get_parameter("open")
                },
            );
            info!(
                "animations: handlers={} stop={} stop_all={} controller={:?}",
                models::register_animation_event("hello:on_step"),
                models::stop("hello:idle"),
                models::stop_all(),
                models::get_animation_controller().parameters().len(),
            );
        });

        // Section 35: HUD elements a mod owns, and the one way this host can show them.
        tasks::schedule_after_ticks(52, move || {
            if !demo(35) {
                return;
            }
            hud::set_screen_size(hud::get_screen_size());
            info!(
                "hud: screen={:?} scaled={:?} safe={:?}",
                hud::get_screen_size(),
                hud::get_scaled_size(2.0),
                hud::get_safe_area(8.0),
            );
            info!(
                "hud: layer={} element={} fps={}",
                hud::register_layer(hud::HudLayer {
                    id: "hello:info".to_owned(),
                    owner: "hello_mod".to_owned(),
                    z_index: 10,
                    visible: true,
                }),
                hud::register_element(
                    hud::HudElement::new("hello:coords", "0, 64, 0")
                        .at(4.0, 4.0)
                        .sized(90.0, 12.0)
                        .alignment(hud::Alignment::TopLeft)
                        .layer("hello:info")
                        .priority(5)
                ),
                hud::register_element(
                    hud::HudElement::new("hello:fps", "60 fps")
                        .sized(60.0, 12.0)
                        .alignment(hud::Alignment::BottomRight)
                        .z(2)
                ),
            );
            info!(
                "hud: elements={:?} bounds_coords={:?} bounds_fps={:?} anchor={:?} z={:?} priority={:?}",
                hud::get_elements()
                    .into_iter()
                    .map(|element| (element.id, element.layer))
                    .collect::<Vec<_>>(),
                hud::get_bounds("hello:coords"),
                hud::get_bounds("hello:fps"),
                hud::get_anchor("hello:coords"),
                hud::get_z_index("hello:fps"),
                hud::get_priority("hello:coords"),
            );
            info!(
                "hud: set_position={} set_size={} set_alignment={} set_anchor={} set_scale={} set_opacity={} set_z={}",
                hud::set_position("hello:coords", 8.0, 8.0),
                hud::set_size("hello:coords", 100.0, 14.0),
                hud::set_alignment("hello:coords", hud::Alignment::TopLeft),
                hud::set_anchor("hello:coords", hud::Anchor::TopLeft),
                hud::set_scale("hello:coords", 1.5),
                hud::set_opacity("hello:coords", 0.8),
                hud::set_z_index("hello:coords", 1),
            );
            info!(
                "hud: bounds_after={:?} contains={} text={} visible={} enabled={}",
                hud::get_bounds("hello:coords"),
                hud::get_element("hello:coords")
                    .is_some_and(|element| element.contains(hud::get_screen_size(), (10.0, 10.0))),
                hud::set_text("hello:coords", "1, 64, -3"),
                hud::set_visible("hello:fps", true),
                hud::set_enabled("hello:fps", true),
            );
            info!(
                "hud: layer={:?} layers={} click={} tooltip={} animation={} invalidate={:?}",
                hud::get_layer("hello:info").map(|layer| layer.z_index),
                hud::get_layers().len(),
                hud::register_click_handler("hello:coords"),
                hud::register_tooltip("hello:coords", "where you are"),
                hud::register_animation("hello:coords"),
                (
                    hud::invalidate_layout(),
                    hud::invalidate_render(),
                    hud::request_redraw(),
                ),
            );
            let (drawn, undrawn) = hud::flush();
            info!(
                "hud: flush drawn={drawn} undrawn={undrawn} known={} refusals={:?}",
                hud::KNOWN_ELEMENTS.len(),
                hud::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "hud: unregister={} layer={} set_layer={}",
                hud::unregister_element("hello:fps"),
                hud::unregister_layer("hello:info"),
                hud::set_layer("hello:coords", "minecraft:default"),
            );
        });

        // Section 36: screens and widgets a mod owns, and the client's screen layer.
        tasks::schedule_after_ticks(54, move || {
            if !demo(36) {
                return;
            }
            info!(
                "ui: register={} settings={} screens={}",
                ui::register_screen(
                    ui::Screen::new("hello:menu", "Hello")
                        .widget(ui::Widget::label("hello:title", "Welcome").sized(200.0, 20.0))
                        .widget(ui::Widget::button("hello:play", "Play").sized(100.0, 20.0))
                        .widget(ui::Widget::label("hello:note", "a note"))
                        .theme("hello:dark")
                        .scale(2.0)
                ),
                ui::register_screen(ui::Screen::new("hello:settings", "Settings")),
                ui::get_screens().len(),
            );
            info!(
                "ui: stack push={} current={:?} previous={:?} stack={:?}",
                ui::push_screen("hello:menu"),
                ui::get_current_screen(),
                ui::get_previous_screen(),
                ui::get_screen_stack(),
            );
            info!(
                "ui: push_settings={} current={:?} previous={:?} pop={:?} current_now={:?}",
                ui::push_screen("hello:settings"),
                ui::get_current_screen(),
                ui::get_previous_screen(),
                ui::pop_screen(),
                ui::get_current_screen(),
            );
            info!(
                "ui: layout absolute={:?}",
                ui::get_layout("hello:menu").map(|(placed, layout)| (layout, placed.len())),
            );
            ui::set_layout("hello:menu", ui::Layout::Vertical);
            info!(
                "ui: layout vertical={:?}",
                ui::get_layout("hello:menu").map(|(placed, _)| placed),
            );
            ui::set_layout("hello:menu", ui::Layout::Grid(2));
            info!(
                "ui: layout grid={:?}",
                ui::get_layout("hello:menu").map(|(placed, _)| placed),
            );
            ui::set_layout("hello:menu", ui::Layout::Absolute);
            info!(
                "ui: widget={:?} focus={} focused={:?} set_focus={} clear={}",
                ui::get_widget("hello:menu", "hello:play").map(|widget| widget.to_string()),
                ui::set_focus("hello:menu", "hello:play"),
                ui::get_focused_widget(),
                ui::set_focus("hello:menu", "hello:title"),
                ui::clear_focus(),
            );
            info!(
                "ui: hover at 50,20={:?} hovered={:?} find={} root={:?} register_widget={}",
                ui::update_hover("hello:menu", (1280.0, 720.0), (50.0, 20.0)),
                ui::get_hovered_widget(),
                ui::find_widget("hello:menu", "hello:").len(),
                ui::get_root_widget("hello:menu"),
                ui::register_widget("hello:menu", ui::Widget::label("hello:extra", "extra")),
            );
            info!(
                "ui: theme={:?} style={} scale={} clip={} cursor={} tooltip={:?}",
                ui::get_theme("hello:menu"),
                ui::set_style("hello:menu", "flat"),
                ui::set_scale("hello:menu", 1.0),
                ui::set_clip_area("hello:menu", (0.0, 0.0, 100.0, 50.0)),
                ui::set_cursor("hello:hand"),
                ui::show_tooltip("press me", 10.0, 20.0),
            );
            info!(
                "ui: input capture={} capturing={} release={} modal={} dialog={} menu={}",
                ui::capture_input(),
                ui::is_capturing_input(),
                ui::release_input(),
                ui::register_modal("hello:menu"),
                ui::register_dialog("hello:menu"),
                ui::register_context_menu("hello:menu"),
            );
            info!(
                "ui: counts={:?} rebuild={} unregister={} widget={}",
                ui::get_counts(),
                ui::rebuild(),
                ui::unregister_screen("hello:settings"),
                ui::unregister_widget("hello:menu", "hello:extra"),
            );
            info!(
                "ui: client-side: open={} close={} replace={} handler={} refusals={:?}",
                ui::open_screen("hello:menu"),
                ui::close_screen(),
                ui::replace_screen("hello:menu"),
                ui::register_widget_handler("hello:menu", "hello:play"),
                ui::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 38: input state a mod keeps, and the client's own devices.
        tasks::schedule_after_ticks(56, move || {
            if !demo(38) {
                return;
            }
            input::clear();
            info!(
                "input: down={} press={} release={} keyboard={:?}",
                input::is_key_down("W"),
                input::is_key_pressed("W"),
                input::is_key_released("W"),
                input::get_keyboard_state(),
            );
            input::press("W");
            info!(
                "input: after press down={} pressed={} released={} device={:?}",
                input::is_key_down("W"),
                input::is_key_pressed("W"),
                input::is_key_released("W"),
                input::get_active_device(),
            );
            input::press("W");
            info!(
                "input: twice={:?}",
                input::get_keyboard_state(),
            );
            input::release("W");
            info!(
                "input: after release down={} pressed={} released={}",
                input::is_key_down("W"),
                input::is_key_pressed("W"),
                input::is_key_released("W"),
            );
            input::clear_edges();
            info!("input: after clear_edges released={}", input::is_key_released("W"));
            input::press("MouseLeft");
            input::press("MouseRight");
            input::move_to(100.0, 50.0);
            input::move_by(10.0, -5.0);
            input::scroll(0.0, 3.0);
            input::set_modifiers(&["Shift", "Control"]);
            info!(
                "input: mouse={:?} position={:?} delta={:?} scroll={:?} left={} right={}",
                input::get_mouse_state(),
                input::get_mouse_position(),
                input::get_mouse_delta(),
                input::get_mouse_scroll(),
                input::is_mouse_down("Left"),
                input::is_mouse_down("Right"),
            );
            info!(
                "input: modifiers={:?} device={:?} last={:?} updates_ok={}",
                input::get_modifiers(),
                input::get_active_device(),
                input::get_last_input_device(),
                input::get_keyboard_state().len() >= 2,
            );
            info!(
                "input: keybind={} labelled={} get={:?} binds={} active={}",
                input::register_keybind("hello:jump", "Space"),
                input::register_keybind_labelled("hello:dash", "Dash", "LeftShift"),
                input::get_keybind("hello:dash").map(|bind| bind.to_string()),
                input::get_keybinds().len(),
                input::is_keybind_active("hello:jump"),
            );
            input::press("Space");
            info!(
                "input: space down active_jump={} active_dash={} rebind={} active_after={}",
                input::is_keybind_active("hello:jump"),
                input::is_keybind_active("hello:dash"),
                input::set_keybind("hello:jump", "J"),
                input::is_keybind_active("hello:jump"),
            );
            info!(
                "input: handler={} handlers={:?} capture={} captured={} release={}",
                input::register_handler("F"),
                input::get_handlers(),
                input::capture_keyboard(),
                input::is_keyboard_captured(),
                input::release_keyboard(),
            );
            info!(
                "input: client-side: controllers={:?} gamepad={:?} touch={:?} grab={:?} grab_pointer={} release={} refusals={:?}",
                input::get_controller_state(),
                input::get_gamepad_state(),
                input::get_touch_state(),
                input::get_pointer_grab(),
                input::grab_pointer(),
                input::release_pointer(),
                input::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "input: unregister={} {} released={:?} clear={:?}",
                input::unregister_keybind("hello:jump"),
                input::unregister_handler("F"),
                {
                    input::release("Space");
                    input::get_keyboard_state()
                },
                {
                    input::clear();
                    input::get_keyboard_state()
                },
            );
        });

        // Section 37: the widget library — a real node tree, real geometry, real events.
        tasks::schedule_after_ticks(58, move || {
            if !demo(37) {
                return;
            }
            widget::register(
                WidgetNode::new("hello:panel", WidgetType::Panel, "")
                    .at(10.0, 10.0)
                    .sized(240.0, 120.0)
                    .with_padding(Edges::all(4.0))
                    .with_border(Edges::all(1.0)),
            );
            widget::register(WidgetNode::new("hello:title", WidgetType::TextLabel, "Settings"));
            widget::register(WidgetNode::new("hello:button", WidgetType::Button, "Apply"));
            widget::register(WidgetNode::new("hello:slider", WidgetType::Slider, ""));
            info!(
                "widget: kinds={} panel={:?} title={:?} button={:?}",
                WidgetType::ALL.len(),
                widget::get_node("hello:panel").map(|node| node.to_widget().to_string()),
                widget::get_node("hello:title").map(|node| node.to_widget().to_string()),
                widget::get_node("hello:button").map(|node| node.to_widget().to_string()),
            );
            info!(
                "widget: kind: text={} children={} value={} ui={:?} id={:?} parent={:?}",
                WidgetType::Button.accepts_text(),
                WidgetType::Panel.accepts_children(),
                WidgetType::Slider.takes_value(),
                WidgetType::Button.ui_kind().as_str(),
                widget::get_id("hello:panel"),
                widget::get_parent("hello:title"),
            );
            info!(
                "widget: add_child={} {} {} cycle={} children={:?}",
                widget::add_child("hello:panel", "hello:title"),
                widget::add_child("hello:panel", "hello:button"),
                widget::add_child("hello:panel", "hello:slider"),
                widget::add_child("hello:title", "hello:panel"),
                widget::get_children("hello:panel"),
            );
            info!(
                "widget: parent_now={:?} margin={} border={} padding={} rect={:?}",
                widget::get_parent("hello:slider"),
                widget::set_margin("hello:title", Edges::sides(2.0, 2.0, 2.0, 2.0)),
                widget::set_border("hello:title", Edges::all(1.0)),
                widget::set_padding("hello:title", Edges::all(3.0)),
                widget::get_rect("hello:title"),
            );
            info!(
                "widget: clamp min={} max={} size={} rect={:?} constraints={} rect2={:?}",
                widget::set_min_size("hello:button", 60.0, 20.0),
                widget::set_max_size("hello:button", 120.0, 40.0),
                widget::set_size("hello:button", 10.0, 5.0),
                widget::get_rect("hello:button"),
                widget::set_constraints(
                    "hello:slider",
                    Constraints::new((10.0, 4.0), (200.0, 30.0)).with_weight(1.0)
                ),
                {
                    widget::set_size("hello:slider", 500.0, 500.0);
                    widget::get_rect("hello:slider")
                },
            );
            info!(
                "widget: layout={} spacing={} placed={:?}",
                widget::set_layout("hello:panel", Layout::Vertical),
                widget::set_spacing("hello:panel", 2.0),
                widget::place("hello:panel"),
            );
            info!(
                "widget: text={:?} set_text={} icon={} tooltip={} visible={} enabled={} disabled={}",
                widget::get_text("hello:title"),
                widget::set_text("hello:title", "General"),
                widget::set_icon("hello:button", "hello:apply"),
                widget::set_tooltip("hello:button", "write the settings"),
                widget::set_visible("hello:title", true),
                widget::set_enabled("hello:button", true),
                widget::set_disabled("hello:slider", false),
            );
            info!(
                "widget: background={} opacity={} rotation={} scale={} cursor={} focusable={} draggable={} resizable={} clippable={}",
                widget::set_background("hello:panel", "hello:panel_bg"),
                widget::set_opacity("hello:panel", 0.9),
                widget::set_rotation("hello:title", 0.0),
                widget::set_scale("hello:title", 1.25),
                widget::set_cursor("hello:button", "hello:hand"),
                widget::set_focusable("hello:button", true),
                widget::set_draggable("hello:panel", true),
                widget::set_resizable("hello:panel", true),
                widget::set_clippable("hello:panel", true),
            );
            widget::set_data("hello:panel", "label", "From the panel");
            info!(
                "widget: data={:?} bind={} read_through={:?} own_wins={:?} unbind={} after={:?}",
                widget::get_data("hello:title", "label"),
                widget::bind("hello:title", "label", "hello:panel"),
                widget::get_data("hello:title", "label"),
                {
                    widget::set_data("hello:title", "label", "Own label");
                    widget::get_data("hello:title", "label")
                },
                widget::unbind("hello:title", "label"),
                widget::get_data("hello:title", "label"),
            );
            info!(
                "widget: handlers={} {} {} {} {} {} {} {} {} {} {}",
                widget::on_click("hello:button", "hello:on_apply"),
                widget::on_double_click("hello:title", "hello:on_rename"),
                widget::on_right_click("hello:button", "hello:on_menu"),
                widget::on_hover("hello:button", "hello:on_hover"),
                widget::on_focus("hello:slider", "hello:on_focus"),
                widget::on_blur("hello:slider", "hello:on_blur"),
                widget::on_change("hello:slider", "hello:on_change"),
                widget::on_submit("hello:slider", "hello:on_submit"),
                widget::on_drag_start("hello:panel", "hello:on_grab"),
                widget::on_drag("hello:panel", "hello:on_drag"),
                widget::on_drag_end("hello:panel", "hello:on_drop"),
            );
            info!(
                "widget: scroll={} value={} submit={} dispatched={} taken={:?}",
                widget::on_scroll("hello:panel", "hello:on_scroll"),
                widget::set_value("hello:slider", "0.5"),
                widget::submit("hello:slider"),
                widget::dispatch("hello:slider", WidgetEvent::Change, "0.75"),
                {
                    let taken = widget::take_dispatches().len();
                    widget::clear_dispatches();
                    taken
                },
            );
            info!(
                "widget: events={} node={:?} value_now={:?} removed={} all={}",
                WidgetEvent::ALL.len(),
                widget::get_node("hello:slider").map(|node| node.to_widget().to_string()),
                widget::get_value("hello:slider"),
                widget::remove_child("hello:panel", "hello:slider"),
                {
                    widget::remove_all_children("hello:panel");
                    widget::get_children("hello:panel").len()
                },
            );
            info!(
                "widget: client-side: draw={} input={} theme={} dialog={} ime={} scroll={} refusals={:?}",
                widget::draw("hello:panel"),
                widget::deliver_input("hello:button", WidgetEvent::Click),
                widget::apply_client_theme("minecraft:dark"),
                widget::open_native_dialog("hello:panel"),
                widget::start_ime("hello:title"),
                widget::scroll_by("hello:panel", 0.0, 4.0),
                widget::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 39: a camera a mod owns, with real matrices and a real frustum.
        tasks::schedule_after_ticks(60, move || {
            if !demo(39) {
                return;
            }
            camera::reset();
            camera::set_viewport(1280, 720);
            camera::set_position(8.0, 65.0, 8.0);
            camera::set_rotation(90.0, -10.0, 0.0);
            info!(
                "camera: position={:?} rotation={:?} direction={:?} up={:?} right={:?}",
                camera::get_position(),
                camera::get_rotation(),
                camera::get_direction(),
                camera::get_up_vector(),
                camera::get_right_vector(),
            );
            info!(
                "camera: fov={} clamped={} near={} far={} mode={:?} aspect={}",
                camera::get_fov(),
                {
                    camera::set_fov(500.0);
                    camera::get_fov()
                },
                camera::get_near_plane(),
                camera::get_far_plane(),
                camera::get_camera_mode(),
                camera::get().aspect(),
            );
            camera::set_fov(70.0);
            camera::set_target("hello:marker");
            camera::set_entity("hello:player");
            info!(
                "camera: look_at={:?} target={:?} entity={:?} third_person={} set={:?}",
                camera::look_at((8.0, 65.0, 0.0)),
                camera::get_target(),
                camera::get_entity(),
                camera::get_third_person_distance(),
                camera::set_third_person_distance(4.0),
            );
            camera::reset();
            camera::set_viewport(1280, 720);
            let forward = camera::get_direction();
            let ahead = (forward.0 * 8.0, forward.1 * 8.0, forward.2 * 8.0);
            let behind = (-ahead.0, -ahead.1, -ahead.2);
            info!(
                "camera: ahead={:?} project={:?} project_behind={:?} unproject={:?}",
                ahead,
                camera::world_to_screen(ahead),
                camera::world_to_screen(behind),
                camera::screen_to_world(640.0, 360.0, 0.5),
            );
            info!(
                "camera: visible={} visible_behind={} box={} contains={} planes={:?}",
                camera::is_visible(ahead),
                camera::is_visible(behind),
                camera::is_box_visible(
                    (ahead.0 - 1.0, ahead.1 - 1.0, ahead.2 - 1.0),
                    (ahead.0 + 1.0, ahead.1 + 1.0, ahead.2 + 1.0)
                ),
                camera::get_frustum().contains_point(ahead),
                {
                    let frustum = camera::get_frustum();
                    (0..6)
                        .filter_map(|index| frustum.plane(index).map(|plane| plane.distance_to(ahead)))
                        .collect::<Vec<_>>()
                },
            );
            camera::shake(0.6);
            info!(
                "camera: shake={} offset={:?} decayed={} offset_after={:?}",
                camera::get_shake(),
                camera::shake_offset(),
                {
                    camera::advance(400);
                    camera::get_shake()
                },
                camera::shake_offset(),
            );
            info!(
                "camera: lerp_position={:?} lerp_rotation={:?} wrap={}",
                camera::lerp_position((0.0, 0.0, 0.0), (10.0, 0.0, 0.0), 0.25),
                camera::lerp_rotation((350.0, 0.0, 0.0), (10.0, 0.0, 0.0), 0.5),
                camera::wrap_degrees(370.0),
            );
            info!(
                "camera: mode={:?} set_bobbing={} roll={} free={} zoom={} modifiers={:?} controller={:?}",
                {
                    camera::set_camera_mode(CameraMode::ThirdPerson);
                    camera::get_camera_mode()
                },
                {
                    camera::set_bobbing(true);
                    camera::get()
                        .direction()
                        .2
                },
                {
                    camera::set_roll(5.0);
                    camera::get_rotation().2
                },
                {
                    camera::set_free_camera(true);
                    camera::get().is_free()
                },
                {
                    camera::set_zoom(2.0);
                    camera::get_zoom()
                },
                {
                    camera::register_modifier("hello:no_shake");
                    camera::get_modifiers()
                },
                {
                    camera::register_camera_controller("hello:orbit");
                    camera::get_camera_controllers()
                },
            );
            info!(
                "camera: client-side: position={:?} rotation={:?} mode={:?} entity={:?} fov={:?} shake={:?}",
                camera::client_position(),
                camera::client_rotation(),
                camera::client_mode(),
                camera::client_entity(),
                camera::client_fov(),
                camera::client_shake(),
            );
            info!(
                "camera: client-side: set_position={} set_rotation={} set_mode={} set_fov={} shake={} modifier={} controller={} refusals={:?}",
                camera::set_client_position(0.0, 0.0, 0.0),
                camera::set_client_rotation(0.0, 0.0, 0.0),
                camera::set_client_mode(CameraMode::FirstPerson),
                camera::set_client_fov(90.0),
                camera::shake_client(0.5),
                camera::call_modifier("hello:no_shake"),
                camera::call_controller("hello:orbit"),
                camera::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
            info!(
                "camera: unregister: modifier={} controller={} reset={:?}",
                camera::unregister_modifier("hello:no_shake"),
                camera::get_camera_controllers().len(),
                {
                    camera::reset();
                    camera::get_position()
                },
            );
        });

        // Section 40: a mixer model a mod owns, and the device it cannot play on.
        tasks::schedule_after_ticks(62, move || {
            if !demo(40) {
                return;
            }
            info!(
                "audio: register={} {} sources={} duration={:?} loaded={} unloaded={}",
                audio::register_sound(
                    Sound::new("hello:ping", "sounds/ping.ogg")
                        .duration(1200)
                        .category(SoundCategory::Player)
                        .volume(0.8)
                        .pitch(1.0)
                        .looping(false)
                ),
                audio::register_sound(
                    Sound::new("hello:music", "sounds/music.ogg")
                        .duration(60000)
                        .category(SoundCategory::Music)
                ),
                audio::get_sounds().len(),
                audio::get_duration("hello:ping"),
                audio::load_sound("hello:ping"),
                audio::unload_sound("hello:music"),
            );
            let bus = audio::create_bus(
                Bus::new("hello:sfx")
                    .volume(0.8)
                    .category(SoundCategory::Player)
                    .reverb(0.2)
                    .filter("lowpass"),
            );
            let source = audio::create_source(
                Source::new("hello:ping")
                    .bus(bus)
                    .volume(0.5)
                    .pitch(1.0)
                    .position([4.0, 64.0, 0.0])
                    .distance(1.0, 16.0)
                    .attenuation(Attenuation::Linear)
                    .spatialization(true)
                    .looping(false),
            );
            info!(
                "audio: bus={} source={} own={:?} resolved={:?} buses={} sources={}",
                audio::get_bus(bus).is_some(),
                audio::get_source(source).is_some(),
                audio::get_volume(source),
                audio::resolve_volume(source),
                audio::get_buses().len(),
                audio::get_sources().len(),
            );
            info!(
                "audio: master={} set_master={} category={} set_category={} categories={}",
                audio::get_master_volume(),
                audio::set_master_volume(0.5),
                audio::get_category_volume(SoundCategory::Player),
                audio::set_category_volume(SoundCategory::Player, 0.5),
                SoundCategory::all().len(),
            );
            audio::set_listener(Listener::new(
                [0.0, 64.0, 0.0],
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
            ));
            info!(
                "audio: listener={:?} right={:?} distance={} attenuation_at={:?} pan_towards={:?} stored_pan={:?}",
                audio::get_listener(),
                audio::get_listener().right(),
                audio::get_listener().distance_to([4.0, 64.0, 0.0]),
                audio::get_source(source).map(|held| held.attenuation_at(4.0)),
                audio::get_source(source).map(|held| held.pan_towards(&audio::get_listener())),
                audio::get_pan(source),
            );
            let playing = audio::play_3d("hello:ping", [4.0, 64.0, 0.0]);
            info!(
                "audio: playing={} state={:?} position={:?}",
                playing.is_some(),
                playing.and_then(audio::get_playback_state),
                playing.and_then(audio::get_playback_position),
            );
            audio::advance(300);
            info!(
                "audio: after_advance={:?} paused={} state={:?} resumed={} stopped={}",
                playing.and_then(audio::get_playback_position),
                playing.map(audio::pause).unwrap_or(false),
                playing.and_then(audio::get_playback_state),
                playing.map(audio::resume).unwrap_or(false),
                playing.map(audio::stop).unwrap_or(false),
            );
            info!(
                "audio: play={} play_2d={} play_at={} play_source={} live={} stop_all={}",
                audio::play("hello:ping").is_some(),
                audio::play_2d("hello:ping").is_some(),
                audio::play_at("hello:ping", [0.0, 64.0, 0.0]).is_some(),
                audio::play_source(source).is_some(),
                audio::get_playbacks().len(),
                audio::stop_all(),
            );
            info!(
                "audio: pitch={:?} set={:?} attenuation={:?} spatial={:?} looping={:?} position={:?}",
                audio::get_pitch(source),
                audio::set_pitch(source, 1.5),
                audio::get_attenuation(source),
                audio::get_spatialization(source),
                audio::get_looping(source),
                audio::set_position(source, [2.0, 64.0, 0.0]),
            );
            info!(
                "audio: reverb={:?} set={} filter={:?} set={} distance={:?} set={:?}",
                audio::get_reverb(bus),
                audio::set_reverb(bus, 0.5),
                audio::get_filter(bus),
                audio::set_filter(bus, "highpass"),
                audio::get_distance(source),
                audio::set_distance(source, 0.5, 24.0),
            );
            info!(
                "audio: client-side: effect={} chain={} devices={:?} set_device={} client_master={:?} set={} client_sounds={:?} decode={}",
                audio::register_effect("hello:echo", "echo"),
                audio::set_effect_chain(bus, &["hello:echo"]),
                audio::get_audio_devices(),
                audio::set_output_device("default"),
                audio::get_client_master_volume(),
                audio::set_client_master_volume(1.0),
                audio::get_client_sounds(),
                audio::load_sound_from_file("hello:from_file", "sounds/ping.ogg"),
            );
            info!(
                "audio: refusals={:?} diagnostics={:?} unregister={}",
                audio::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
                audio::get_diagnostics(),
                audio::unregister_sound("hello:music"),
            );
        });

        // Section 41: particles a mod really simulates, and the drawing it cannot do.
        tasks::schedule_after_ticks(64, move || {
            if !demo(41) {
                return;
            }
            info!(
                "particles: register={} type={:?} types={}",
                particles::register(
                    ParticleType::new("hello:spark")
                        .sprite("hello:spark")
                        .size(0.2)
                        .color([1.0, 0.8, 0.2, 1.0])
                        .gravity(-9.8)
                        .velocity((0.5, 2.0, 0.0))
                        .blend(BlendMode::Additive)
                        .max_age(2.0)
                        .lifetime(1.0)
                ),
                particles::get_type("hello:spark").map(|kind| (
                    kind.id.clone(),
                    kind.sprite.clone(),
                    kind.size,
                    kind.max_age,
                )),
                particles::get_types().len(),
            );
            let one = particles::spawn_at("hello:spark", (0.0, 0.0, 0.0));
            info!(
                "particles: spawn={:?} at={:?} velocity={:?} count={}",
                one,
                one.and_then(particles::get).map(|held| held.position),
                one.and_then(particles::get).map(|held| held.velocity),
                particles::get_count(),
            );
            particles::tick(0.5);
            info!(
                "particles: after 0.5s position={:?} velocity={:?} age={:?}",
                one.and_then(particles::get).map(|held| held.position),
                one.and_then(particles::get).map(|held| held.velocity),
                one.and_then(particles::get).map(|held| held.age),
            );
            particles::tick(1.6);
            info!(
                "particles: after 2.1s alive={} (max age 2.0 removed it)",
                particles::get(one.unwrap_or(0)).is_some(),
            );
            let burst = particles::spawn_in_box(
                "hello:spark",
                (-1.0, -1.0, -1.0),
                (1.0, 1.0, 1.0),
                5,
                42,
            );
            particles::tick(0.75);
            let live = particles::get_active();
            let all_inside = !live.is_empty()
                && live.iter().all(|held| {
                    held.position.0 >= -1.0
                        && held.position.0 <= 1.0
                        && held.position.1 >= -1.0
                        && held.position.1 <= 1.0
                        && held.position.2 >= -1.0
                        && held.position.2 <= 1.0
                });
            info!(
                "particles: box burst={} alive_after_0.75s={} inside_box={}",
                burst.len(),
                live.len(),
                all_inside,
            );
            info!(
                "particles: line={} sphere={} burst_repeatable={:?}",
                particles::spawn_along_line("hello:spark", (0.0, 0.0, 0.0), (4.0, 0.0, 0.0), 3).len(),
                particles::spawn_in_sphere("hello:spark", (0.0, 64.0, 0.0), 2.0, 4, 9).len(),
                (
                    particles::spawn_burst("hello:spark", (0.0, 64.0, 0.0), 2, 1.0, 11)
                        .into_iter()
                        .filter_map(particles::get)
                        .map(|held| held.velocity)
                        .collect::<Vec<_>>(),
                    particles::spawn_burst("hello:spark", (0.0, 64.0, 0.0), 2, 1.0, 11)
                        .into_iter()
                        .filter_map(particles::get)
                        .map(|held| held.velocity)
                        .collect::<Vec<_>>(),
                    particles::spawn_burst("hello:spark", (0.0, 64.0, 0.0), 2, 1.0, 12)
                        .into_iter()
                        .filter_map(particles::get)
                        .map(|held| held.velocity)
                        .collect::<Vec<_>>(),
                ),
            );
            let held = particles::spawn_at("hello:spark", (0.0, 0.0, 0.0));
            info!(
                "particles: fade={:?} set_position={} set_velocity={} set_gravity={} set_collision={}",
                held.and_then(particles::get)
                    .map(|particle| (particle.alpha_at(0.0), particle.alpha_at(0.5), particle.alpha_at(1.0))),
                held.map(|h| particles::set_position(h, (1.0, 2.0, 3.0))).unwrap_or(false),
                held.map(|h| particles::set_velocity(h, (0.0, 0.0, 0.0))).unwrap_or(false),
                held.map(|h| particles::set_gravity(h, 0.0)).unwrap_or(false),
                held.map(|h| particles::set_collision(h, true)).unwrap_or(false),
            );
            info!(
                "particles: color={} scale={} lifetime={} removes={} clear={}",
                held.map(|h| particles::set_color(h, [1.0, 0.0, 0.0, 1.0])).unwrap_or(false),
                held.map(|h| particles::set_scale(h, 2.0)).unwrap_or(false),
                held.map(|h| particles::set_lifetime(h, 4.0)).unwrap_or(false),
                held.map(particles::remove).unwrap_or(false),
                particles::clear(),
            );
            let template = particles::register_emitter(
                Emitter::new("hello:smoke", "hello:spark")
                    .rate(2.5)
                    .at((0.0, 64.0, 0.0))
                    .direction((0.0, 1.0, 0.0))
                    .spread(15.0)
                    .speed(0.5)
                    .shape(EmissionShape::Sphere { radius: 1.0 })
                    .max_alive(50)
                    .seed(7),
            );
            let emitter = particles::create_emitter("hello:smoke");
            let first_second = particles::advance_emitters(1.0);
            let second_second = particles::advance_emitters(1.0);
            info!(
                "particles: emitter={template} handle={:?} at_1s={first_second} at_2s={second_second} emitter_total={:?} emitters={}",
                emitter,
                emitter.and_then(particles::get_emitter).map(|held| held.spawned()),
                particles::get_emitters().len(),
            );
            info!(
                "particles: emitter_control rate={:?} shape={:?} pause={:?} resume={:?} stop={:?}",
                emitter.map(|handle| particles::set_emission_rate(handle, 4.0)),
                emitter.map(|handle| particles::set_emission_shape(
                    handle,
                    EmissionShape::Box { size: (2.0, 2.0, 2.0) }
                )),
                emitter.map(particles::pause_emitter),
                emitter.map(particles::resume_emitter),
                emitter.map(particles::stop_emitter),
            );
            info!(
                "particles: unregister_emitter={} effects={} types={}",
                particles::unregister_emitter("hello:smoke"),
                particles::get_effects().len(),
                particles::get_types().len(),
            );
            info!(
                "particles: effect={} effects={} client-side: texture={} animation={} blend={} renderer={} list={} world={:?} refusals={:?}",
                particles::register_effect(
                    VisualEffect::new("hello:poof")
                        .emitter("hello:smoke")
                        .kind("hello:spark")
                        .duration(3)
                        .looping(true)
                ),
                particles::get_effects().len(),
                particles::set_texture(0, "hello:spark"),
                particles::set_animation(0, "hello:flicker"),
                particles::set_blend_mode(0, BlendMode::Alpha),
                particles::register_particle_renderer("hello:renderer"),
                particles::client_particles().len(),
                particles::spawn_into_world("hello:spark", (0.0, 64.0, 0.0)),
                particles::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 42: a socket a mod really opens, and the client's connection it cannot see.
        tasks::schedule_after_ticks(66, move || {
            if !demo(42) {
                return;
            }
            network::set_timeout(200);
            let listener = network::listen("127.0.0.1:0");
            info!(
                "network: listen={:?} bound={:?} address={:?}",
                listener,
                listener.and_then(network::listener_address),
                network::Address::new("127.0.0.1", 25565),
            );
            info!(
                "network: closed_port={:?} udp={:?} connection={:?}",
                network::connect("127.0.0.1:1"),
                network::bind_udp("127.0.0.1:0"),
                network::bind_udp("127.0.0.1:0")
                    .and_then(network::connection)
                    .map(|held| (held.transport, held.state, held.timeout)),
            );
            info!(
                "network: channel={} {} channels={:?} message={} {} messages_ok={}",
                network::register_channel("hello:chat"),
                network::register_channel("hello:state"),
                network::get_channels(),
                network::register_message("hello:hello"),
                network::register_message("hello:hello"),
                network::get_channels().len() >= 2,
            );
            info!(
                "network: handlers channel={} handler={} handlers={} traffic={:?}",
                network::register_channel("hello:events"),
                network::register_handler("hello:chat"),
                network::get_handlers().len(),
                network::get_traffic_stats(),
            );
            info!(
                "network: settings compression={} encryption={} rate_limit={:?} retry={:?}",
                {
                    network::set_compression(true);
                    network::set_rate_limit(65536);
                    true
                },
                {
                    network::set_encryption(false);
                    true
                },
                network::set_timeout(500),
                network::RetryPolicy::Fixed {
                    attempts: 3,
                    delay_millis: 250,
                },
            );
            info!(
                "network: close={:?} stopped={:?} unregister={} {}",
                network::bind_udp("127.0.0.1:0").map(network::close),
                listener.map(network::stop_listening),
                network::unregister_channel("hello:state"),
                network::unregister_message("hello:hello"),
            );
            info!(
                "network: client-side: connected={} disconnected={} connection={:?} connections={} remote={:?} local={:?}",
                network::is_connected(),
                network::is_disconnected(),
                network::get_connection().map(|held| held.label),
                network::get_connections().len(),
                network::get_remote_address(),
                network::get_local_address(),
            );
            info!(
                "network: client-side: latency={:?} ping={:?} jitter={:?} loss={:?} bandwidth={:?} state={:?}",
                network::get_latency(),
                network::get_ping(),
                network::get_jitter(),
                network::get_packet_loss(),
                network::get_bandwidth(),
                network::get_connection_state(),
            );
            info!(
                "network: client-side: protocol={:?} session={:?} disconnect={} kick={} send={} refusals={:?}",
                network::get_protocol_version(),
                network::get_session_id(),
                network::disconnect_client(),
                network::kick_player("someone"),
                network::send_to_client(&[1, 2, 3]),
                network::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 43: a real packet codec, and the client's protocol it cannot touch.
        tasks::schedule_after_ticks(68, move || {
            if !demo(43) {
                return;
            }
            let mut buffer = packets::Buffer::new();
            buffer.write_u8(7);
            buffer.write_u16(500);
            buffer.write_u32(70_000);
            buffer.write_varint(300);
            buffer.write_string("hello");
            buffer.write_f32(1.5);
            buffer.write_bool(true);
            buffer.write_bytes(&[1, 2, 3]);
            let written = buffer.len();
            info!(
                "packets: wrote={} position={} remaining={}",
                written,
                buffer.position(),
                buffer.remaining(),
            );
            let mut reader = packets::Buffer::from_slice(buffer.bytes());
            info!(
                "packets: read back u8={:?} u16={:?} u32={:?} varint={:?} string={:?} f32={:?} bool={:?} bytes={:?}",
                reader.read_u8(),
                reader.read_u16(),
                reader.read_u32(),
                reader.read_varint(),
                reader.read_string(),
                reader.read_f32(),
                reader.read_bool(),
                reader.read_bytes(3),
            );
            info!(
                "packets: overrun={:?} position_now={}",
                reader.read_u32(),
                reader.position(),
            );
            let payload = [9u8, 8, 7];
            let frame = packets::encode(&payload);
            info!(
                "packets: frame={:?} decoded={:?} truncated={:?}",
                frame,
                packets::decode(&frame),
                packets::decode(&frame[..frame.len() - 1]),
            );
            info!(
                "packets: varint edges={:?}",
                [0u32, 127, 128, 255, 16_383, 16_384, u32::MAX]
                    .iter()
                    .map(|value| {
                        let mut buffer = packets::Buffer::new();
                        buffer.write_varint(*value);
                        let bytes = buffer.into_bytes();
                        let mut reader = packets::Buffer::from_slice(&bytes);
                        (bytes.len(), reader.read_varint().ok())
                    })
                    .collect::<Vec<_>>(),
            );
            let packet = packets::Packet::new(0x7f, "hello:ping")
                .direction(packets::Direction::ToServer)
                .state(packets::PacketState::Play)
                .payload(&[1, 2, 3])
                .metadata("owner", "hello_mod");
            info!(
                "packets: register={:?} duplicate={:?} id={:?} direction={:?} size={:?} codec={}",
                packets::register(packet.clone()),
                packets::register(packet.clone()),
                packets::get_packet_id("hello:ping"),
                packets::get_direction(0x7f),
                packets::get_size(0x7f),
                packets::register_codec(packets::Codec::new(0x7f, "hello:codec")),
            );
            let text = packets::serialize(&packet);
            info!(
                "packets: serialize={} inspect={:?} round_trip={:?} validate={:?}",
                text.len(),
                packets::inspect(&packet),
                packets::deserialize(&text).map(|held| held.name),
                packets::validate(&packet),
            );
            let handler = packets::register_handler(0x7f, Some(packets::Direction::ToServer), |_| true);
            let mut dispatchee = packet.clone();
            info!(
                "packets: handler={handler} dispatched={} feed={:?} trace={} cleared={}",
                packets::dispatch(&mut dispatchee),
                packets::feed(0x7f, &packets::encode(&[4, 5, 6])),
                {
                    packets::set_trace_enabled(true);
                    packets::get_trace().len()
                },
                packets::clear_trace(),
            );
            info!(
                "packets: registry={} supports={} unregister_handler={} unregister={} trace_off={}",
                packets::get_registry().len(),
                packets::supports_packet(0x7f),
                packets::unregister_handler(handler),
                packets::unregister(0x7f),
                packets::set_trace_enabled(false),
            );
            info!(
                "packets: client-side: cancel={} replace={} modify={} send={} to_server={} to_client={} to_all={}",
                packets::cancel(0x7f),
                packets::replace(&packet),
                packets::modify(&mut packet.clone()),
                packets::send(&packet),
                packets::send_to_server(&packet),
                packets::send_to_client(&packet),
                packets::send_to_all(&packet),
            );
            info!(
                "packets: client-side: observer={} unknown={:?} protocol={:?} mapping={:?} claim={} events={} refusals={:?}",
                packets::set_observer(|_| {}),
                packets::get_unknown_packets().len(),
                packets::get_protocol_version(),
                packets::get_protocol_mapping("hello:ping"),
                packets::register_protocol_mapping("hello:ping", 0x7f),
                packets::PACKET_EVENTS.len(),
                packets::refusals().into_iter().map(|(what, _)| what).collect::<Vec<_>>(),
            );
        });

        // Section 5: what this mod holds, what it does not, and why.
        info!(
            "capabilities: known={} held={:?} render={} world_read={} unsupported_why={}",
            capabilities::list().len(),
            capabilities::list()
                .into_iter()
                .filter(|capability| capabilities::has(*capability))
                .collect::<Vec<_>>(),
            capabilities::has(Capability::Render),
            capabilities::has(Capability::WorldRead),
            capabilities::why_unsupported(Capability::WorldWrite),
        );
        info!(
            "capabilities: describe(render)={:?} v{} experimental(shader)={} unsafe(filesystem_write)={} requirements={:?}",
            capabilities::describe(Capability::Render),
            capabilities::get_version(Capability::Render),
            capabilities::is_experimental(Capability::ShaderAccess),
            capabilities::is_unsafe(Capability::FilesystemWrite),
            capabilities::get_requirements(Capability::FilesystemWrite),
        );
        info!(
            "capabilities: get(world_write)={:?} get(nonsense)={:?} permissions_missing={:?} providers(filesystem_read)={:?}",
            capabilities::get("world_write"),
            capabilities::get("nonsense"),
            capabilities::check_permissions(),
            capabilities::provided_by_id(Capability::FilesystemRead),
        );
        if capabilities::require(Capability::WorldRead) {
            info!("capabilities: world_read granted");
        }
        // Section 4: who else is loaded, what needs what, and the order the loader chose.
        info!(
            "deps: order={:?} mine={:?} core_loaded={} core_enabled={} dependents={:?}",
            mods::get_load_order(),
            mods::get_dependencies("hello_mod"),
            mods::is_loaded("core_lib"),
            mods::is_enabled("core_lib"),
            mods::get_dependents("core_lib"),
        );
        info!(
            "deps: why={:?} missing={:?} errors={:?} cycles={}",
            mods::explain_load_order(),
            mods::get_missing_dependencies(),
            mods::get_load_errors(),
            mods::find_cycles().len(),
        );
        for entry in mods::all() {
            info!(
                "deps: {} ({}) v{} {} enabled={} active={}",
                entry.name, entry.id, entry.version, entry.state, entry.enabled, entry.is_active(),
            );
        }
    }

    fn tick(&mut self, dt_millis: u32) {
        let tick = self.ticks.fetch_add(1, Ordering::Relaxed) + 1;
        TICKS.store(tick, Ordering::Relaxed);
        // A background result is picked up on the tick, never waited for.
        if let Some(result) = self.pool_result.as_ref() {
            if let Some(text) = result.get() {
                info!("tasks: pool returned {text:?}");
                self.pool_result = None;
            }
        }
        if let Some(worker) = self.worker.as_ref() {
            if let Some((sum, thread, main)) = worker.get() {
                info!("tasks: worker summed to {sum} on {thread} (main={main})");
                self.worker = None;
            }
        }
        if tick == 240 {
            info!(
                "tasks: cancelling the repeat, metrics={:?}",
                tasks::get_metrics()
            );
            tasks::cancel(self.beat_task);
        }
        // Twice through the tick counter: the mod switches itself off, and core_lib
        // switches it back on, which is how the enable and disable events get delivered.
        if tick == 5 {
            info!(
                "events: switching myself off, pending={} cancelled={}",
                events::pending_count(),
                events::is_cancelled()
            );
            mods::disable("hello_mod");
        }
        if !self.beat.ready(dt_millis) {
            return;
        }
        let seconds = tick / 60;
        if enabled("watermark") {
            overlay_line(&format!(
                "cinnaloader: hello-mod online ({seconds}s, x{:.1}, {})",
                number("scale"),
                choice_index("corner"),
            ));
        }
        info!("hello-mod alive: {seconds}s (+{} ms)", dt_millis);
    }

    fn migrate(&mut self, from: &ModVersion, to: &ModVersion) {
        info!("lifecycle: migrate {from} -> {to} (saved data is mine to move)");
    }

    fn initialize(&mut self) {
        info!(
            "capabilities: register(command_registration)={} held={}",
            capabilities::register(Capability::CommandRegistration),
            capabilities::has(Capability::CommandRegistration),
        );
        info!("lifecycle: initialize (phase {})", current_phase());
    }

    fn post_initialize(&mut self) {
        info!("lifecycle: post_initialize");
    }

    fn ready(&mut self) {
        info!("lifecycle: ready, initialized={} ready={}", is_initialized(), is_ready());
    }

    fn config_loaded(&mut self) {
        info!("lifecycle: config_loaded (saved settings are in effect)");
    }

    fn tick_start(&mut self, _dt_millis: u32) {
        if self.ticks.load(Ordering::Relaxed) == 1 {
            info!("lifecycle: first tick_start");
        }
    }

    fn render_start(&mut self) {
        if self.frames.fetch_add(1, Ordering::Relaxed) == 0 {
            info!("lifecycle: first frame (render_start), phase {}", current_phase());
        }
    }

    fn error(&mut self, kind: &str, message: &str) {
        info!("lifecycle: caught {kind}: {message}");
    }

    fn toggle(&mut self, id: &str, on: bool) {
        info!("hello-mod: {id} is now {on}");
    }

    fn slider(&mut self, id: &str, value: f32) {
        info!("hello-mod: {id} = {value:.2}");
    }

    fn choice(&mut self, id: &str, index: usize) {
        info!("hello-mod: {id} = option {index}");
    }

    fn keybind(&mut self, id: &str, down: bool) {
        if id == "panic" && down {
            info!("hello-mod: panic, everything off");
            set_enabled("watermark", false);
        }
    }

    fn pressed(&mut self, id: &str) {
        info!("hello-mod: {id} pressed");
    }

    fn unload(&mut self) {
        info!("hello-mod unloading");
        overlay_line("cinnaloader: hello-mod offline");
    }

    fn shutdown(&mut self) {
        info!("lifecycle: shutdown");
    }
}

export_mod!(Hello, manifest = ModManifest::new("hello_mod", "Hello mod", env!("CARGO_PKG_VERSION"))
    .description("The worked example: one of every setting kind")
    .author("cinnaloader")
    .tag("example")
    // A mod asking to be trusted is clamped by the loader, and this one says so out loud.
    .trust(TrustLevel::Trusted)
    .capability(Capability::FilesystemRead)
    .capability(Capability::FilesystemWrite)
    .capability(Capability::Render)
    .capability(Capability::PlayerRead)
    .permission(Permission::ReadFiles)
    .permission(Permission::WriteFiles)
    .permission(Permission::ModifyRendering)
    .permission(Permission::ReadPlayerData)
    .depends_on_versioned("core_lib", ">=0.1")
    .load_after("core_lib")
    .recommends("missing_lib")
    .requires_api(8)
    .requires_loader(">=0.1, <1"));
