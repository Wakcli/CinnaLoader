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
