//! Example Cinnaloader mod: one of every setting kind, a keybind and a timer.

use std::sync::atomic::{AtomicU32, Ordering};

use cinnaloader::prelude::*;

#[derive(Default)]
struct Hello {
    ticks: AtomicU32,
    /// Fires about once a second, wherever the loader ticks it.
    beat: Timer,
    /// Frames seen, so the render hooks can prove themselves once instead of every frame.
    frames: AtomicU32,
}

impl Mod for Hello {
    fn load(&mut self) {
        declare(
            ModManifest::new("hello-mod", "Hello mod", env!("CARGO_PKG_VERSION"))
                .description("The worked example: one of every setting kind")
                .author("cinnaloader")
                .contributors(&["you"])
                .credit("the Cinnabar agent docs")
                .license("MIT")
                .website("https://github.com/Wakcli/CinnaLoader")
                .source("https://github.com/Wakcli/CinnaLoader")
                .tag("example")
                .tag("settings")
                .category("Display")
                .environment(Environment::Client)
                .capability(Capability::Files)
                .capability(Capability::PlayerRead)
                .permission(Permission::WorldState)
                // Requirements, checked by the loader before this mod is loaded.
                .requires_api(7)
                .requires_loader(">=0.1, <1")
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
            "manifest v{} | tags {:?} | capabilities {} | loader v{} | abi {}",
            manifest_version(),
            tags(),
            capabilities().len(),
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
    }

    fn tick(&mut self, dt_millis: u32) {
        let tick = self.ticks.fetch_add(1, Ordering::Relaxed) + 1;
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

export_mod!(Hello);
