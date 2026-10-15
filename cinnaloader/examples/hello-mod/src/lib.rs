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
        // A posted event waits for the end of the tick; a published one runs now.
        events::post(events::Event::new("hello:ready").with_text("queued during load"));

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
        // A hook that cannot run yet, and one that runs as this mod is dropped.
        defer_until_ready(|| info!("lifecycle: deferred hook ran once the loader was ready"));
        register_cleanup(|| info!("lifecycle: cleanup ran while unloading"));
        // Whatever the user last chose, kept next to the mods.
        if let Some(name) = config("name") {
            info!("hello-mod remembered {name}");
        } else {
            set_config("name", "hello-mod");
        }
        info!("hello-mod initialised (mods from {})", mods_dir().unwrap_or("?"));
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
    .capability(Capability::FilesystemRead)
    .capability(Capability::FilesystemWrite)
    .capability(Capability::Render)
    .capability(Capability::PlayerRead)
    .permission(Permission::ReadFiles)
    .permission(Permission::WriteFiles)
    .permission(Permission::Render)
    .permission(Permission::PlayerState)
    .permission(Permission::WorldState)
    .depends_on_versioned("core_lib", ">=0.1")
    .load_after("core_lib")
    .recommends("missing_lib")
    .requires_api(8)
    .requires_loader(">=0.1, <1"));
