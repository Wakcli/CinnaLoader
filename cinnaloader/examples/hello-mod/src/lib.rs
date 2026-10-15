//! Example Cinnaloader mod: one of every setting kind, a keybind and a timer.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cinnaloader::prelude::*;

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
        tasks::submit(move || {
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
