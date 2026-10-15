//! Input: the state a mod can keep for itself, and the client's own devices.
//!
//! The loader reads the keyboard and the mouse with `device_query` and hands a mod the key
//! events it sees (sections 1 and 6: a keybind's press and release arrive as events). This
//! module turns those into the state a mod asks about: which keys are down, which were just
//! pressed or released, the mouse buttons, the pointer's position and how far it moved, the
//! scroll, and the modifiers that are held. A mod updates it with [`press`], [`release`] and
//! [`move_to`] — which the SDK's own event loop does — and reads the answers.
//!
//! Keybinds are real: registering one, rebinding it, listing them and asking whether one is
//! active are all values a mod owns, and they are the same names the loader's settings use.
//!
//! What is out of reach is the client's own input: what the *client* thinks is pressed, its
//! pointer grab, its controllers and its touch state are inside it, and the loader only sees
//! what X11 gives it — which is why a headless run cannot exercise a key at all. Those answer
//! honestly rather than guessing.
//!
//! ```
//! use cinnaloader::prelude::*;
//!
//! input::press("W");
//! assert!(input::is_key_down("W"));
//! assert!(input::is_key_pressed("W"));
//! input::release("W");
//! assert!(input::is_key_released("W"));
//! input::register_keybind("hello:jump", "Space");
//! input::press("Space");
//! assert!(input::is_keybind_active("hello:jump"));
//! ```

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, OnceLock};

/// A key or button name, as the loader spells it: `W`, `Space`, `MouseLeft`.
pub type Button = String;

/// Which device the input came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Device {
    #[default]
    Keyboard,
    Mouse,
    Gamepad,
    Touch,
    /// Something the loader cannot name.
    Unknown,
}

impl Device {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
            Self::Gamepad => "gamepad",
            Self::Touch => "touch",
            Self::Unknown => "unknown",
        }
    }
}

/// The state the loader has seen.
#[derive(Debug, Default, Clone)]
pub struct InputState {
    down: Vec<Button>,
    pressed: Vec<Button>,
    released: Vec<Button>,
    pointer: (f64, f64),
    previous_pointer: (f64, f64),
    scroll: (f64, f64),
    modifiers: Vec<String>,
    device: Device,
    last_device: Device,
    /// How many updates it has taken, for a caller that wants to know it moved.
    updates: u64,
}

impl InputState {
    /// Whether a key or button is down right now.
    #[must_use]
    pub fn is_down(&self, button: &str) -> bool {
        self.down.iter().any(|held| held == button)
    }

    /// Whether it went down since the last [`InputState::clear_edges`].
    #[must_use]
    pub fn is_pressed(&self, button: &str) -> bool {
        self.pressed.iter().any(|held| held == button)
    }

    /// Whether it came up since then.
    #[must_use]
    pub fn is_released(&self, button: &str) -> bool {
        self.released.iter().any(|held| held == button)
    }

    /// Everything that is down.
    #[must_use]
    pub fn down_list(&self) -> Vec<Button> {
        self.down.clone()
    }

    /// Where the pointer is.
    #[must_use]
    pub fn pointer(&self) -> (f64, f64) {
        self.pointer
    }

    /// How far it moved since the last update.
    #[must_use]
    pub fn delta(&self) -> (f64, f64) {
        (
            self.pointer.0 - self.previous_pointer.0,
            self.pointer.1 - self.previous_pointer.1,
        )
    }

    /// How far it has scrolled.
    #[must_use]
    pub fn scroll(&self) -> (f64, f64) {
        self.scroll
    }

    /// The modifiers that are held.
    #[must_use]
    pub fn modifiers(&self) -> Vec<String> {
        self.modifiers.clone()
    }

    #[must_use]
    pub fn device(&self) -> Device {
        self.device
    }

    #[must_use]
    pub fn last_device(&self) -> Device {
        self.last_device
    }

    #[must_use]
    pub fn updates(&self) -> u64 {
        self.updates
    }

    /// Forgets the edges, which a caller does once it has acted on them.
    pub fn clear_edges(&mut self) {
        self.pressed.clear();
        self.released.clear();
        self.scroll = (0.0, 0.0);
    }

    /// Forgets everything, as if no key had ever been pressed.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

fn state() -> &'static Mutex<InputState> {
    static STATE: OnceLock<Mutex<InputState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(InputState::default()))
}

fn with<T>(body: impl FnOnce(&mut InputState) -> T) -> T {
    let mut guard = state().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Notes that a key or button went down. The SDK's event loop calls this.
pub fn press(button: &str) {
    with(|state| {
        state.updates += 1;
        state.device = if button.starts_with("Mouse") {
            Device::Mouse
        } else if button.starts_with("Pad") {
            Device::Gamepad
        } else if button.starts_with("Touch") {
            Device::Touch
        } else {
            Device::Keyboard
        };
        state.last_device = state.device;
        if !state.down.iter().any(|held| held == button) {
            state.down.push(button.to_owned());
            state.pressed.push(button.to_owned());
            state.released.retain(|held| held != button);
        }
    });
}

/// Notes that it came up.
pub fn release(button: &str) {
    with(|state| {
        state.updates += 1;
        state.down.retain(|held| held != button);
        if !state.released.iter().any(|held| held == button) {
            state.released.push(button.to_owned());
        }
        state.pressed.retain(|held| held != button);
    });
}

/// Notes where the pointer is.
pub fn move_to(x: f64, y: f64) {
    with(|state| {
        state.updates += 1;
        state.previous_pointer = state.pointer;
        state.pointer = (x, y);
        state.device = Device::Mouse;
        state.last_device = Device::Mouse;
    });
}

/// Notes how far the pointer moved, adding to where it is.
pub fn move_by(dx: f64, dy: f64) {
    let (x, y) = with(|state| state.pointer);
    move_to(x + dx, y + dy);
}

/// Notes a scroll.
pub fn scroll(dx: f64, dy: f64) {
    with(|state| {
        state.updates += 1;
        state.scroll = (state.scroll.0 + dx, state.scroll.1 + dy);
        state.device = Device::Mouse;
        state.last_device = Device::Mouse;
    });
}

/// Sets the modifiers that are held.
pub fn set_modifiers(modifiers: &[&str]) {
    with(|state| {
        state.modifiers = modifiers.iter().map(|name| (*name).to_owned()).collect();
    });
}

/// Forgets the edges.
pub fn clear_edges() {
    with(InputState::clear_edges);
}

/// Forgets everything.
pub fn clear() {
    with(InputState::clear);
}

/// The whole state, for a caller that wants to hold it.
#[must_use]
pub fn get_keyboard_state() -> Vec<Button> {
    with(|state| state.down_list())
}

/// Whether a key is down.
#[must_use]
pub fn is_key_down(key: &str) -> bool {
    with(|state| state.is_down(key))
}

/// Whether it went down since the last clear.
#[must_use]
pub fn is_key_pressed(key: &str) -> bool {
    with(|state| state.is_pressed(key))
}

/// Whether it came up since then.
#[must_use]
pub fn is_key_released(key: &str) -> bool {
    with(|state| state.is_released(key))
}

/// Whether a mouse button is down.
#[must_use]
pub fn is_mouse_down(button: &str) -> bool {
    with(|state| state.is_down(&format!("Mouse{button}")))
}

/// Whether it went down.
#[must_use]
pub fn is_mouse_pressed(button: &str) -> bool {
    with(|state| state.is_pressed(&format!("Mouse{button}")))
}

/// Whether it came up.
#[must_use]
pub fn is_mouse_released(button: &str) -> bool {
    with(|state| state.is_released(&format!("Mouse{button}")))
}

/// Where the pointer is.
#[must_use]
pub fn get_mouse_position() -> (f64, f64) {
    with(|state| state.pointer())
}

/// How far it moved.
#[must_use]
pub fn get_mouse_delta() -> (f64, f64) {
    with(|state| state.delta())
}

/// How far it scrolled.
#[must_use]
pub fn get_mouse_scroll() -> (f64, f64) {
    with(|state| state.scroll())
}

/// The mouse's state, as `(buttons down, position)`.
#[must_use]
pub fn get_mouse_state() -> (Vec<Button>, (f64, f64)) {
    with(|state| {
        let buttons = state
            .down_list()
            .into_iter()
            .filter(|button| button.starts_with("Mouse"))
            .collect();
        (buttons, state.pointer())
    })
}

/// The modifiers that are held.
#[must_use]
pub fn get_modifiers() -> Vec<String> {
    with(|state| state.modifiers())
}

/// Which device the last input came from.
#[must_use]
pub fn get_active_device() -> Device {
    with(|state| state.device())
}

/// The same, under the name the wishlist also uses.
#[must_use]
pub fn get_last_input_device() -> Device {
    with(|state| state.last_device())
}

fn refused(what: &str) -> bool {
    crate::warn!("a mod asked this host about input: {}", why_unknown(what));
    false
}

/// The client's controller state. Empty: the loader does not read controllers.
#[must_use]
pub fn get_controller_state() -> Vec<(String, f64)> {
    Vec::new()
}

/// The same, under the name the wishlist also uses.
#[must_use]
pub fn get_gamepad_state() -> Vec<(String, f64)> {
    Vec::new()
}

/// The client's touch state. Empty.
#[must_use]
pub fn get_touch_state() -> Vec<(u32, (f64, f64))> {
    Vec::new()
}

/// Why an input question has no answer here.
#[must_use]
pub fn why_unknown(what: &str) -> String {
    match what {
        "device" => {
            "the loader reads the keyboard and the mouse through X11, and only while the game \
             window has focus; the client's own devices, its pointer grab and its controllers \
             are inside it"
                .to_owned()
        }
        "capture" => {
            "capturing the keyboard would mean stopping the client from seeing it, which \
             needs its input path; a mod's own capture flag is what this host keeps"
                .to_owned()
        }
        "controller" => "the loader does not read controllers: device_query does not expose them".to_owned(),
        "touch" => "a touch screen is the client's, and this host sees no touch events".to_owned(),
        _ => format!("{what} is not something this host can see"),
    }
}

/// What this host cannot reach in this section.
#[must_use]
pub fn refusals() -> Vec<(String, String)> {
    ["device", "capture", "controller", "touch"]
        .iter()
        .map(|what| ((*what).to_owned(), why_unknown(what)))
        .collect()
}

/// One keybinding a mod registered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybind {
    pub id: String,
    pub owner: String,
    pub label: String,
    pub key: Button,
    /// The key this loader's settings registered first, for a mod that rebinds.
    pub default_key: Button,
}

impl fmt::Display for Keybind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({}) -> {}", self.label, self.id, self.key)
    }
}

fn keybinds() -> &'static Mutex<BTreeMap<String, Keybind>> {
    static KEYBINDS: OnceLock<Mutex<BTreeMap<String, Keybind>>> = OnceLock::new();
    KEYBINDS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn with_keybinds<T>(body: impl FnOnce(&mut BTreeMap<String, Keybind>) -> T) -> T {
    let mut guard = keybinds().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Registers a keybinding. The loader's own settings register one the same way, so a mod's
/// binding shows up in its panel.
pub fn register_keybind(id: &str, key: &str) -> bool {
    with_keybinds(|keybinds| {
        if keybinds.contains_key(id) {
            return false;
        }
        keybinds.insert(
            id.to_owned(),
            Keybind {
                id: id.to_owned(),
                owner: crate::metadata::id(),
                label: id.to_owned(),
                key: key.to_owned(),
                default_key: key.to_owned(),
            },
        );
        true
    })
}

/// The same, with a label a panel shows.
pub fn register_keybind_labelled(id: &str, label: &str, key: &str) -> bool {
    let registered = register_keybind(id, key);
    if registered {
        with_keybinds(|keybinds| {
            if let Some(bind) = keybinds.get_mut(id) {
                bind.label = label.to_owned();
            }
        });
    }
    registered
}

/// Forgets one.
pub fn unregister_keybind(id: &str) -> bool {
    with_keybinds(|keybinds| keybinds.remove(id).is_some())
}

/// A keybinding by id.
#[must_use]
pub fn get_keybind(id: &str) -> Option<Keybind> {
    with_keybinds(|keybinds| keybinds.get(id).cloned())
}

/// Rebinds one, and answers whether it was there.
pub fn set_keybind(id: &str, key: &str) -> bool {
    with_keybinds(|keybinds| match keybinds.get_mut(id) {
        Some(bind) => {
            bind.key = key.to_owned();
            true
        }
        None => false,
    })
}

/// Every keybinding a mod registered.
#[must_use]
pub fn get_keybinds() -> Vec<Keybind> {
    with_keybinds(|keybinds| keybinds.values().cloned().collect())
}

/// Whether a keybinding's key is down right now.
#[must_use]
pub fn is_keybind_active(id: &str) -> bool {
    match get_keybind(id) {
        Some(bind) => is_key_down(&bind.key),
        None => false,
    }
}

/// Records that a mod has a handler for a key, which the loader's own key polling would
/// deliver as a keybind event.
pub fn register_handler(key: &str) -> bool {
    with_handlers(|handlers| {
        if handlers.contains(&key.to_owned()) {
            false
        } else {
            handlers.push(key.to_owned());
            true
        }
    })
}

/// Forgets one.
pub fn unregister_handler(key: &str) -> bool {
    with_handlers(|handlers| {
        let before = handlers.len();
        handlers.retain(|held| held != key);
        handlers.len() != before
    })
}

/// The handlers a mod registered.
#[must_use]
pub fn get_handlers() -> Vec<String> {
    with_handlers(|handlers| handlers.clone())
}

fn handlers() -> &'static Mutex<Vec<String>> {
    static HANDLERS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    HANDLERS.get_or_init(|| Mutex::new(Vec::new()))
}

fn with_handlers<T>(body: impl FnOnce(&mut Vec<String>) -> T) -> T {
    let mut guard = handlers().lock().unwrap_or_else(|error| error.into_inner());
    body(&mut guard)
}

/// Captures the keyboard for a mod's own screen, which is a flag this host keeps: the client
/// still sees the keys, because stopping that needs its input path.
pub fn capture_keyboard() -> bool {
    with(|state| {
        let was = state.modifiers.contains(&"captured".to_owned());
        if !was {
            state.modifiers.push("captured".to_owned());
        }
        !was
    })
}

/// Gives it back.
pub fn release_keyboard() -> bool {
    with(|state| {
        let before = state.modifiers.len();
        state.modifiers.retain(|held| held != "captured");
        state.modifiers.len() != before
    })
}

/// Whether a mod has captured the keyboard.
#[must_use]
pub fn is_keyboard_captured() -> bool {
    with(|state| state.modifiers().iter().any(|held| held == "captured"))
}

/// The client's pointer grab. `None`: the client owns it, and this host does not pretend to
/// know.
#[must_use]
pub fn get_pointer_grab() -> Option<bool> {
    None
}

/// Asks the client to grab the pointer. `false`.
pub fn grab_pointer() -> bool {
    refused("capture")
}

/// Asks it to let go. `false`.
pub fn release_pointer() -> bool {
    refused("capture")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn fresh() {
        clear();
        with_keybinds(|keybinds| keybinds.clear());
        with_handlers(|handlers| handlers.clear());
    }

    #[test]
    fn a_key_press_and_release_are_edges_the_caller_clears() {
        let _serial = serial();
        fresh();
        assert!(!is_key_down("W"));
        press("W");
        assert!(is_key_down("W"));
        assert!(is_key_pressed("W"));
        assert!(!is_key_released("W"));
        press("W");
        assert_eq!(get_keyboard_state().len(), 1, "pressing twice is still one key down");
        release("W");
        assert!(!is_key_down("W"));
        assert!(is_key_released("W"));
        assert!(!is_key_pressed("W"), "the press edge is gone");
        clear_edges();
        assert!(!is_key_released("W"), "and so is the release");
        press("Space");
        press("MouseLeft");
        assert_eq!(get_keyboard_state().len(), 2);
        assert_eq!(get_active_device(), Device::Mouse, "the last one was the mouse");
        assert_eq!(get_last_input_device(), Device::Mouse);
        assert!(is_mouse_down("Left"));
        release("MouseLeft");
        assert!(is_mouse_released("Left"));
        assert!(is_mouse_pressed("Left") || is_mouse_released("Left"));
        clear();
        assert!(get_keyboard_state().is_empty());
        assert_eq!(get_active_device(), Device::Keyboard);
        assert_eq!(Device::Gamepad.as_str(), "gamepad");
        assert_eq!(Device::Touch.as_str(), "touch");
        press("PadA");
        assert_eq!(get_active_device(), Device::Gamepad);
        release("PadA");
        press("Touch0");
        assert_eq!(get_active_device(), Device::Touch);
        release("Touch0");
        press("Something");
        assert_eq!(get_active_device(), Device::Keyboard);
    }

    #[test]
    fn the_pointer_moves_scrolls_and_remembers_where_it_was() {
        let _serial = serial();
        fresh();
        assert_eq!(get_mouse_position(), (0.0, 0.0));
        move_to(100.0, 50.0);
        assert_eq!(get_mouse_position(), (100.0, 50.0));
        assert_eq!(get_mouse_delta(), (100.0, 50.0));
        move_by(10.0, -5.0);
        assert_eq!(get_mouse_position(), (110.0, 45.0));
        assert_eq!(get_mouse_delta(), (10.0, -5.0));
        scroll(0.0, 3.0);
        assert_eq!(get_mouse_scroll(), (0.0, 3.0));
        scroll(1.0, 1.0);
        assert_eq!(get_mouse_scroll(), (1.0, 4.0));
        clear_edges();
        assert_eq!(get_mouse_scroll(), (0.0, 0.0));
        assert_eq!(
            get_mouse_delta(),
            (10.0, -5.0),
            "clearing the edges does not forget where the pointer is"
        );
        move_to(110.0, 45.0);
        assert_eq!(get_mouse_delta(), (0.0, 0.0), "moving to the same place is no movement");
        assert_eq!(get_active_device(), Device::Mouse);
        assert!(state().lock().unwrap().updates() > 0);
    }

    #[test]
    fn mouse_buttons_and_modifiers_read_the_way_a_caller_expects() {
        let _serial = serial();
        fresh();
        press("MouseLeft");
        press("MouseRight");
        press("W");
        let (buttons, position) = get_mouse_state();
        assert_eq!(buttons, vec!["MouseLeft".to_owned(), "MouseRight".to_owned()]);
        assert_eq!(position, (0.0, 0.0));
        assert!(is_mouse_down("Left"));
        assert!(is_mouse_down("Right"));
        assert!(!is_mouse_down("Middle"));
        set_modifiers(&["Shift", "Control"]);
        assert_eq!(get_modifiers(), vec!["Shift".to_owned(), "Control".to_owned()]);
        clear();
        assert!(get_modifiers().is_empty());
    }

    #[test]
    fn a_keybind_can_be_registered_rebound_and_asked_about() {
        let _serial = serial();
        fresh();
        assert!(register_keybind("hello:jump", "Space"));
        assert!(!register_keybind("hello:jump", "Space"), "once");
        assert!(register_keybind_labelled("hello:dash", "Dash", "LeftShift"));
        let bind = get_keybind("hello:dash").expect("registered");
        assert_eq!(bind.label, "Dash");
        assert_eq!(bind.key, "LeftShift");
        assert_eq!(bind.default_key, "LeftShift");
        assert_eq!(get_keybinds().len(), 2);
        assert!(bind.to_string().contains("Dash"));

        assert!(!is_keybind_active("hello:jump"));
        press("Space");
        assert!(is_keybind_active("hello:jump"));
        assert!(!is_keybind_active("hello:dash"));
        assert!(!is_keybind_active("nothing"));
        assert!(set_keybind("hello:jump", "J"));
        assert!(!is_keybind_active("hello:jump"), "it was rebound away from Space");
        assert!(!set_keybind("nothing", "K"));
        assert_eq!(get_keybind("hello:jump").map(|bind| bind.key), Some("J".to_owned()));
        assert!(unregister_keybind("hello:jump"));
        assert!(!unregister_keybind("hello:jump"));
        assert_eq!(get_keybinds().len(), 1);
        release("Space");
    }

    #[test]
    fn handlers_and_the_capture_flag_are_the_mods_own() {
        let _serial = serial();
        fresh();
        assert!(register_handler("F"));
        assert!(!register_handler("F"));
        assert_eq!(get_handlers(), vec!["F".to_owned()]);
        assert!(unregister_handler("F"));
        assert!(!unregister_handler("F"));

        assert!(!is_keyboard_captured());
        assert!(capture_keyboard());
        assert!(!capture_keyboard(), "capturing twice says so");
        assert!(is_keyboard_captured());
        assert!(release_keyboard());
        assert!(!release_keyboard());
        assert!(!is_keyboard_captured());
    }

    #[test]
    fn the_clients_devices_and_pointer_grab_are_refused() {
        let _serial = serial();
        fresh();
        assert!(get_controller_state().is_empty());
        assert!(get_gamepad_state().is_empty());
        assert!(get_touch_state().is_empty());
        assert!(get_pointer_grab().is_none());
        assert!(!grab_pointer());
        assert!(!release_pointer());
        assert_eq!(refusals().len(), 4);
        for (what, why) in refusals() {
            assert!(!why.is_empty(), "{what} has no reason");
            assert_ne!(why, format!("{what} is not something this host can see"));
        }
        assert!(why_unknown("device").contains("window has focus"));
    }
}
