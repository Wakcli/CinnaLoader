//! Launch-time runtime for the Cinnaloader loader.
//!
//! Built as a cdylib and placed on `LD_PRELOAD` (Linux) when the loader launches the
//! target, so the constructor below runs inside the target process at startup. It
//! loads the mod cdylibs, drives them from a background tick thread, and draws the
//! loader menu in-frame by hooking the client's own F3 overlay publication.

mod ingame;
mod panel;

use std::path::PathBuf;

use cinnaloader_loader::Host;

/// Environment variable naming the mods directory; defaults to `./cinnaloader`.
pub const MODS_DIR_ENV: &str = "CINNALOADER_DIR";
/// Set by the first process to load the runtime; children inherit it and skip loading.
const ACTIVE_ENV: &str = "CINNALOADER_ACTIVE";
#[ctor::ctor]
fn start() {
    if std::env::var_os(ACTIVE_ENV).is_some() || !primary_process() {
        return;
    }
    std::env::set_var(ACTIVE_ENV, "1");
    prefer_x11();

    cinnaloader_loader::set_host_facts(host_facts());

    let dir = std::env::var(MODS_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("cinnaloader"));
    let host = match Host::load_dir(&dir) {
        Ok(host) => host,
        Err(error) => {
            eprintln!("[cinnaloader] runtime could not start: {error:#}");
            return;
        }
    };
    eprintln!(
        "[cinnaloader] runtime started: {} mod(s) from {}",
        host.len(),
        dir.display()
    );

    // Before the client's threads start: the inline patches rewrite live code, so they
    // must not be written while another thread is inside the target function. The
    // targeted symbol lookup keeps this from holding up startup.
    ingame::install();
    // The client's own frames drive the mods: the frame hook ticks them and brackets each
    // frame with the render phases, so there is no second clock to keep in step.
    cinnaloader_loader::hand_over(host);
}

/// Pins the target to X11 when Xwayland is available.
///
/// `device_query`, which the menu reads keys with, reads the X keyboard; a Wayland
/// client's key presses never reach it. `CINNALOADER_KEEP_WAYLAND` skips this.
fn prefer_x11() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("CINNALOADER_KEEP_WAYLAND").is_some()
            || std::env::var_os("WAYLAND_DISPLAY").is_none()
            || std::env::var_os("DISPLAY").is_none()
        {
            return;
        }
        std::env::set_var("WINIT_UNIX_BACKEND", "x11");
        std::env::remove_var("WAYLAND_DISPLAY");
        eprintln!("[cinnaloader] pinned to X11 so the menu keybind can read the keyboard");
    }
}

/// What the client is, as far as this process can tell.
///
/// The game version is not a symbol the client exposes, so it is read from the Bedrock
/// samples the client was built against, next to its binary; `CINNALOADER_GAME_VERSION`
/// overrides it. An empty version means "unknown", which a requirement treats as
/// uncheckable rather than failed.
fn host_facts() -> cinnaloader::modinfo::HostFacts {
    cinnaloader::modinfo::HostFacts {
        game_version: std::env::var("CINNALOADER_GAME_VERSION")
            .ok()
            .filter(|version| !version.is_empty())
            .unwrap_or_else(detect_game_version),
        game_build: String::new(),
        game_edition: "bedrock".to_owned(),
        game_platform: std::env::consts::OS.to_owned(),
        game_protocol: None,
        api_version: cinnaloader::ABI_VERSION,
        loader_version: env!("CARGO_PKG_VERSION").to_owned(),
        features: ["files", "player_read", "render", "input"]
            .iter()
            .map(|name| (*name).to_owned())
            .collect(),
    }
}

/// The Bedrock version the client's own samples name, or empty when they are not there.
fn detect_game_version() -> String {
    let Ok(exe) = std::fs::read_link("/proc/self/exe") else {
        return String::new();
    };
    for root in exe.ancestors().skip(1) {
        let downloads = root.join(".local/assets/downloads");
        if let Ok(entries) = std::fs::read_dir(&downloads) {
            for entry in entries.flatten() {
                if let Some(version) = samples_version(&entry.file_name().to_string_lossy()) {
                    eprintln!("[cinnaloader] client game version {version} from {}", entry.path().display());
                    return version;
                }
            }
        }
        let samples = root.join(".local/assets/bedrock-samples");
        if let Ok(entries) = std::fs::read_dir(&samples) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let version = name.strip_prefix('v').unwrap_or(&name);
                if version.starts_with(char::is_numeric) {
                    eprintln!("[cinnaloader] client game version {version} from {}", entry.path().display());
                    return version.to_owned();
                }
            }
        }
    }
    String::new()
}

/// `bedrock-samples-v1.26.50.4-full.zip` names version `1.26.50.4`.
fn samples_version(file_name: &str) -> Option<String> {
    let rest = file_name.strip_prefix("bedrock-samples-v")?;
    let version = rest.strip_suffix("-full.zip")?;
    (!version.is_empty()).then(|| version.to_owned())
}

/// Only the client binary itself, not the helper processes it spawns.
fn primary_process() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link("/proc/self/exe")
            .ok()
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .is_some_and(|name| name == "cinnabar")
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}
