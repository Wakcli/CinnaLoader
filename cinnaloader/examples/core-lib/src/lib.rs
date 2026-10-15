//! Example dependency: a small library another mod names in its manifest, so the loader
//! has an edge to sort on.

use std::sync::atomic::{AtomicU32, Ordering};

use cinnaloader::prelude::*;

#[derive(Default)]
struct Core {
    ticks: AtomicU32,
}

impl Mod for Core {
    fn load(&mut self) {
        info!("core_lib: load, order is {:?}", mods::get_load_order());
    }

    fn ready(&mut self) {
        info!("core_lib: ready, hello_mod enabled={}", mods::is_enabled("hello_mod"));
    }

    fn initialize(&mut self) {
        feature("core:on", "Core", "Library", true);
        info!(
            "core_lib: initialize, dependents {:?}, provides {} loaded={}",
            mods::get_dependents("core_lib"),
            mods::get_dependency_graph().provides.len(),
            mods::is_loaded("core_lib"),
        );
    }

    fn tick(&mut self, _dt_millis: u32) {
        // hello_mod switched itself off a moment ago; switch it back on, so its own bus
        // gets the disable and enable events it is owed.
        let ticks = self.ticks.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks == 120 {
            info!("core_lib: switching hello_mod back on");
            mods::enable("hello_mod");
        }
    }
}

export_mod!(Core, manifest = ModManifest::new("core_lib", "Core library", env!("CARGO_PKG_VERSION"))
    .description("The mod hello-mod declares a dependency on")
    .author("cinnaloader")
    .license("MIT")
    .tag("example")
    .provides("core_lib")
    .capability(Capability::FilesystemRead)
    .permission(Permission::ReadFiles)
    .requires_api(8)
    .requires_loader(">=0.1, <1"));
