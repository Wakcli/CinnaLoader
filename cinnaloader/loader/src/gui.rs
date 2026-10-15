//! The ClickGUI: an always-on-top egui window of mod toggles with a global keybind.

use std::time::Duration;

use device_query::{DeviceQuery, DeviceState, Keycode};
use eframe::egui;

use crate::Host;

/// The global keys that toggle the GUI, polled each frame.
const TOGGLE_KEYS: [Keycode; 2] = [Keycode::LShift, Keycode::RShift];

/// Runs the ClickGUI. When `host` is `Some`, the GUI ticks the mods; otherwise the
/// caller ticks them elsewhere (the in-client runtime owns them on another thread).
/// `start_visible` opens the window immediately instead of waiting for the keybind.
pub fn run(host: Option<Host>, start_visible: bool) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([340.0, 260.0])
            .with_title("cinnaloader")
            .with_visible(start_visible)
            .with_always_on_top(),
        // The GUI runs on a background thread inside the target, so the event loop must
        // accept a non-main thread (otherwise winit panics).
        event_loop_builder: Some(Box::new(|builder| {
            #[cfg(target_os = "linux")]
            {
                use winit::platform::wayland::EventLoopBuilderExtWayland;
                EventLoopBuilderExtWayland::with_any_thread(builder, true);
            }
            #[cfg(target_os = "linux")]
            {
                use winit::platform::x11::EventLoopBuilderExtX11;
                EventLoopBuilderExtX11::with_any_thread(builder, true);
            }
            let _ = builder;
        })),
        ..Default::default()
    };
    eframe::run_native(
        "cinnaloader",
        options,
        Box::new(move |_cc| {
            Ok(Box::new(ClickGui {
                host,
                device: DeviceState::new(),
                held: Vec::new(),
                enabled: Vec::new(),
                visible: start_visible,
            }))
        }),
    )
    .map_err(|error| error.to_string())
}

struct ClickGui {
    host: Option<Host>,
    device: DeviceState,
    held: Vec<Keycode>,
    enabled: Vec<bool>,
    visible: bool,
}

impl eframe::App for ClickGui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // A fresh press of a toggle key — or a mod's `toggle_gui()` — flips visibility.
        let keys = self.device.get_keys();
        let pressed = TOGGLE_KEYS.iter().any(|key| keys.contains(key));
        let was_pressed = TOGGLE_KEYS.iter().any(|key| self.held.contains(key));
        if (pressed && !was_pressed) || crate::take_gui_toggle() {
            self.visible = !self.visible;
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Visible(self.visible));
        }
        self.held = keys;

        if !self.visible {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
            return;
        }

        if let Some(host) = self.host.as_mut() {
            host.tick(16);
        }
        let names = crate::mod_names();
        ui.heading("cinnaloader");
        ui.label(format!("toggle: Shift   mods: {}", names.len()));
        ui.separator();
        if self.enabled.len() != names.len() {
            self.enabled = vec![true; names.len()];
        }
        for (index, name) in names.iter().enumerate() {
            ui.checkbox(&mut self.enabled[index], name);
        }
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for line in crate::overlay_lines() {
                ui.label(line);
            }
        });
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    }
}
