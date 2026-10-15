//! Example of a mod the loader refuses before `load` ever runs: it needs a mod that is not
//! installed. Nothing in this file executes, which is the point.

use cinnaloader::prelude::*;

#[derive(Default)]
struct NeedsMissing;

impl Mod for NeedsMissing {
    fn load(&mut self) {
        info!("this line must never appear: the loader refused the mod");
    }
}

export_mod!(NeedsMissing, manifest = ModManifest::new(
    "needs_missing",
    "Needs something absent",
    env!("CARGO_PKG_VERSION"),
)
.description("Would not load: it requires absent_lib")
.author("cinnaloader")
.license("MIT")
.tag("example")
.requires_api(8)
.requires_loader(">=0.1, <1")
.depends_on("absent_lib"));
