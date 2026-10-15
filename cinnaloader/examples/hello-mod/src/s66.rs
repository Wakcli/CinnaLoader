//! Section 66: the window facts a launcher provides, and the platform this host runs on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

pub fn run() -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(format!(
        "cinnaloader::platform os/arch/family = {}/{}/{}, id = {}, cores = {}, little_endian = {}, 64-bit = {}",
        cinnaloader::platform::get_os(),
        cinnaloader::platform::get_architecture(),
        cinnaloader::platform::get_family(),
        cinnaloader::platform::platform_id(),
        cinnaloader::platform::cpu_count(),
        cinnaloader::platform::is_little_endian(),
        cinnaloader::platform::is_64_bit(),
    ));

    lines.push(format!(
        "cinnaloader::platform::is_supported Cpu/Runtime/Environment/Paths/PageSize/Gpu/Clipboard = {}/{}/{}/{}/{}/{}/{}",
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::Cpu),
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::Runtime),
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::Environment),
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::Paths),
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::PageSize),
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::Gpu),
        cinnaloader::platform::is_supported(cinnaloader::platform::PlatformCapability::Clipboard),
    ));

    let paths = cinnaloader::platform::get_paths();
    lines.push(format!(
        "cinnaloader::platform::get_paths app = {:?}, home = {:?}, temp = {:?}, writable = {}",
        paths.app, paths.home, paths.temp, paths.writable,
    ));

    cinnaloader::window::set_size(640, 480);
    let physical = cinnaloader::window::to_physical(120.0, 2.0);
    lines.push(format!(
        "cinnaloader::window dpi to_physical(120.0, 2.0) = {physical}, to_logical = {}, rounding 1.1 at scale 1.5 = {}",
        cinnaloader::window::to_logical(physical, 2.0),
        cinnaloader::window::to_physical(1.1, 1.5),
    ));

    let resizes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&resizes);
    let listener = cinnaloader::window::on_resized(move |event| {
        assert_eq!(event.pair(), Some((1024.0, 768.0)));
        counter.fetch_add(1, Ordering::SeqCst);
    });
    cinnaloader::window::set_size(1024, 768);
    cinnaloader::window::set_size(1024, 768);
    let fired = resizes.load(Ordering::SeqCst);
    let _ = cinnaloader::events::unsubscribe(listener);
    lines.push(format!(
        "cinnaloader::window::on_resized fired {fired} time(s) for 1024x768 then the same again, aspect = {:?}",
        cinnaloader::window::get_aspect_ratio(),
    ));

    let cursor = cinnaloader::window::set_cursor(5000.0, -3.0);
    lines.push(format!(
        "cinnaloader::window::set_cursor(5000.0, -3.0) clamped to {:?} in a {:?} window, cursor now {:?}",
        cursor,
        cinnaloader::window::get_size(),
        cinnaloader::window::get_cursor(),
    ));

    cinnaloader::window::set_monitors(vec![
        cinnaloader::window::Monitor {
            name: "left".to_owned(),
            width: 1280,
            height: 720,
            scale_factor: 1.0,
            refresh_rate: 60,
            primary: false,
        },
        cinnaloader::window::Monitor {
            name: "main".to_owned(),
            width: 2560,
            height: 1440,
            scale_factor: 2.0,
            refresh_rate: 144,
            primary: true,
        },
    ]);
    let monitor = cinnaloader::window::get_monitor().expect("a primary monitor was set");
    lines.push(format!(
        "cinnaloader::window::get_monitor {} {}x{}@{}Hz primary = {}, scale_factor = {}, refresh_rate = {:?}",
        monitor.name,
        monitor.width,
        monitor.height,
        monitor.refresh_rate,
        monitor.primary,
        cinnaloader::window::get_scale_factor(),
        cinnaloader::window::get_refresh_rate(),
    ));

    for (what, why) in cinnaloader::window::refusals() {
        lines.push(format!("cinnaloader::window refused {what}: {why}"));
    }
    for (what, why) in cinnaloader::platform::refusals() {
        lines.push(format!("cinnaloader::platform refused {what}: {why}"));
    }

    lines
}
