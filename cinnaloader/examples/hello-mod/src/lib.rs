//! Example Cinnaloader mod: one of every setting kind, a keybind and a timer.

use std::sync::atomic::{AtomicU32, Ordering};

use cinnaloader::prelude::*;

#[derive(Default)]
struct Hello {
    ticks: AtomicU32,
    /// Fires about once a second, wherever the loader ticks it.
    beat: Timer,
}

impl Mod for Hello {
    fn load(&mut self) {
        meta!("Hello mod", env!("CARGO_PKG_VERSION"), "cinnaloader");
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
        info!("hello-mod shutting down");
        overlay_line("cinnaloader: hello-mod offline");
    }
}

export_mod!(Hello);
