//! The loader's in-game menu.
//!
//! Two surfaces, picked at install time:
//!
//! * A client-rendered panel (`panel.rs`) — the real ClickGUI. The client paints it,
//!   gives it the pointer and reports clicks back. Needs a client built with the
//!   `local-mods` feature, since that is what keeps the panel API linked.
//! * Otherwise the client's F3 diagnostics panel, driven with our own rows. Text only,
//!   but it works on any client build.
//!
//! Both are reached by hooking `capture_hud_frame`, which runs every frame and hands us
//! `&mut UiPresentationRuntime` in rcx. Keys are polled with `device_query`, which reads
//! the X keyboard, so the client is pinned to X11 (see `prefer_x11`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use cinnaloader_loader as loader;
use cinnaloader_native::detour::{hook_closure_jmp_back, CallbackOption, HookFlags, Registers};
use cinnaloader_native::symbols;
use device_query::Keycode;

use crate::panel;

const CAPTURE_SYMBOL: &str = "client_ui::ui_runtime::presentation::publish::capture_hud_frame";
const SET_SYMBOL: &str =
    "<client_ui::ui_runtime::presentation::UiPresentationRuntime>::swap_debug_lines";

/// Frames a key stays ignored after acting, while it is still held.
const REPEAT_GUARD: u32 = 10;

/// Same layout as `client_ui::ui_runtime::presentation::DebugLines`.
#[repr(C)]
struct DebugLines {
    left: Vec<String>,
    right: Vec<String>,
}

/// `unsafe extern "sysv64" fn(&mut UiPresentationRuntime, Option<DebugLines>)` (byval).
type SetDebugLines = unsafe extern "sysv64" fn(*mut u8, *mut u8);

static OPEN: AtomicBool = AtomicBool::new(false);
static CURSOR: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    /// When the previous frame started, so a tick knows how long it has been.
    static FRAME_STARTED: std::cell::Cell<std::time::Instant> =
        std::cell::Cell::new(std::time::Instant::now());
}

/// The symbols the menu needs, as substrings of their mangled names.
const NEEDLES: [&str; 5] = [
    "capture_hud_frame",
    "swap_debug_lines",
    "mod_ui_mut",
    "set_mod_panel",
    "mod_panel_events",
];

/// Installs the menu, if the client exposes what it needs.
pub fn install() {
    let Ok(syms) = symbols::current_exe_symbols_matching(&NEEDLES) else {
        eprintln!("[cinnaloader] menu: cannot read the client symbol table");
        return;
    };
    let Ok(base) = symbols::load_base() else {
        eprintln!("[cinnaloader] menu: cannot find the client load base");
        return;
    };
    let Some(capture) = syms.offset(CAPTURE_SYMBOL) else {
        eprintln!("[cinnaloader] menu: capture_hud_frame missing from the client");
        return;
    };
    let panel = panel::install(&syms, base);
    let (set_debug, address) = match (panel, syms.offset(SET_SYMBOL)) {
        (true, _) => (None, base + capture as usize),
        (false, Some(set)) => {
            let set = unsafe { std::mem::transmute::<usize, SetDebugLines>(base + set as usize) };
            (Some(set), base + capture as usize)
        }
        (false, None) => {
            eprintln!("[cinnaloader] menu: no panel API and no debug lines; menu disabled");
            return;
        }
    };

    let hook = unsafe {
        hook_closure_jmp_back(
            address,
            move |regs: *mut Registers| {
                let now = std::time::Instant::now();
                let dt = now.saturating_duration_since(FRAME_STARTED.with(|slot| {
                    let previous = slot.get();
                    slot.set(now);
                    previous
                }));
                loader::frame(u32::try_from(dt.as_millis()).unwrap_or(u32::MAX));
                let presentation = (*regs).rcx as *mut u8;
                if presentation.is_null() {
                    return;
                }
                if panel {
                    panel::sync(presentation, &loader::settings());
                } else if OPEN.load(Ordering::Relaxed) {
                    let Some(set) = set_debug else {
                        return;
                    };
                    let names = loader::mod_names();
                    let cursor = CURSOR.load(Ordering::Relaxed);
                    let mut lines: Option<DebugLines> = Some(DebugLines::menu(&names, cursor));
                    set(presentation, &mut lines as *mut _ as *mut u8);
                    // The callee took ownership of the Option through the pointer.
                    std::mem::forget(lines);
                }
            },
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    match hook {
        Ok(hook) => {
            std::mem::forget(hook);
            eprintln!(
                "[cinnaloader] menu hooked at {:#x} ({})",
                base + capture as usize,
                if panel { "client panel" } else { "debug lines" }
            );
        }
        Err(error) => eprintln!("[cinnaloader] menu hook failed: {error:?}"),
    }
    std::thread::spawn(move || poll_keys(panel));
    // Opens the menu without a key press, for headless captures and scripted clicks.
    if std::env::var_os("CINNALOADER_MENU_OPEN").is_some() {
        if panel {
            panel::set_open(true);
        }
        OPEN.store(true, Ordering::SeqCst);
    }
}

impl DebugLines {
    /// The two columns of the fallback menu.
    fn menu(names: &[String], cursor: usize) -> Self {
        let mut left = Vec::with_capacity(names.len() + 3);
        let mut right = Vec::new();
        left.push(format!("cinnaloader  |  {} mod(s)", names.len()));
        left.push(String::new());
        if names.is_empty() {
            left.push("  (no mods loaded)".to_string());
        }
        for (index, name) in names.iter().enumerate() {
            let marker = if index == cursor { ">" } else { " " };
            let check = if loader::mod_enabled(name) { "x" } else { " " };
            left.push(format!("  {marker} [{check}] {name}"));
        }
        right.push("Shift: close".to_string());
        right.push("Up/Down: move".to_string());
        right.push("Enter: toggle".to_string());
        Self { left, right }
    }
}


/// A key name a mod can bind, matching the names the loader compares against. Lowercase,
/// and positional like a US keyboard (`semicolon`, not `;`).
fn key_name(key: Keycode) -> Option<&'static str> {
    Some(match key {
        Keycode::Key0 => "0",
        Keycode::Key1 => "1",
        Keycode::Key2 => "2",
        Keycode::Key3 => "3",
        Keycode::Key4 => "4",
        Keycode::Key5 => "5",
        Keycode::Key6 => "6",
        Keycode::Key7 => "7",
        Keycode::Key8 => "8",
        Keycode::Key9 => "9",
        Keycode::A => "a",
        Keycode::B => "b",
        Keycode::C => "c",
        Keycode::D => "d",
        Keycode::E => "e",
        Keycode::F => "f",
        Keycode::G => "g",
        Keycode::H => "h",
        Keycode::I => "i",
        Keycode::J => "j",
        Keycode::K => "k",
        Keycode::L => "l",
        Keycode::M => "m",
        Keycode::N => "n",
        Keycode::O => "o",
        Keycode::P => "p",
        Keycode::Q => "q",
        Keycode::R => "r",
        Keycode::S => "s",
        Keycode::T => "t",
        Keycode::U => "u",
        Keycode::V => "v",
        Keycode::W => "w",
        Keycode::X => "x",
        Keycode::Y => "y",
        Keycode::Z => "z",
        Keycode::F1 => "f1",
        Keycode::F2 => "f2",
        Keycode::F3 => "f3",
        Keycode::F4 => "f4",
        Keycode::F5 => "f5",
        Keycode::F6 => "f6",
        Keycode::F7 => "f7",
        Keycode::F8 => "f8",
        Keycode::F9 => "f9",
        Keycode::F10 => "f10",
        Keycode::F11 => "f11",
        Keycode::F12 => "f12",
        Keycode::Escape => "escape",
        Keycode::Space => "space",
        Keycode::LControl => "lcontrol",
        Keycode::RControl => "rcontrol",
        Keycode::LShift => "lshift",
        Keycode::RShift => "rshift",
        Keycode::LAlt => "lalt",
        Keycode::RAlt => "ralt",
        Keycode::Enter => "enter",
        Keycode::NumpadEnter => "numpadenter",
        Keycode::Up => "up",
        Keycode::Down => "down",
        Keycode::Left => "left",
        Keycode::Right => "right",
        Keycode::Backspace => "backspace",
        Keycode::Tab => "tab",
        Keycode::Home => "home",
        Keycode::End => "end",
        Keycode::PageUp => "pageup",
        Keycode::PageDown => "pagedown",
        Keycode::Insert => "insert",
        Keycode::Delete => "delete",
        Keycode::CapsLock => "capslock",
        Keycode::Grave => "grave",
        Keycode::Minus => "minus",
        Keycode::Equal => "equal",
        Keycode::LeftBracket => "leftbracket",
        Keycode::RightBracket => "rightbracket",
        Keycode::BackSlash => "backslash",
        Keycode::Semicolon => "semicolon",
        Keycode::Apostrophe => "apostrophe",
        Keycode::Comma => "comma",
        Keycode::Dot => "dot",
        Keycode::Slash => "slash",
        Keycode::Numpad0 => "numpad0",
        Keycode::Numpad1 => "numpad1",
        Keycode::Numpad2 => "numpad2",
        Keycode::Numpad3 => "numpad3",
        Keycode::Numpad4 => "numpad4",
        Keycode::Numpad5 => "numpad5",
        Keycode::Numpad6 => "numpad6",
        Keycode::Numpad7 => "numpad7",
        Keycode::Numpad8 => "numpad8",
        Keycode::Numpad9 => "numpad9",
        _ => return None,
    })
}

/// The menu's key loop: Shift opens and closes it and Escape closes it; Up/Down move the
/// highlighted row and Enter presses it, because the panel's own pointer input is not
/// wired to settings yet. Held keys also go to the loader, which turns them into
/// keybind presses and releases for the mods.
fn poll_keys(panel: bool) {
    use device_query::{DeviceQuery, DeviceState};
    let mut device = None;
    let mut prev: Vec<Keycode> = Vec::new();
    let mut shift_guard = 0u32;
    let mut enter_guard = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(16));
        if device.is_none() {
            device = DeviceState::checked_new();
            continue;
        }
        let keys = device
            .as_ref()
            .map(DeviceQuery::get_keys)
            .unwrap_or_default();
        let pressed: Vec<String> = keys
            .iter()
            .filter_map(|key| key_name(*key))
            .map(str::to_owned)
            .collect();
        loader::update_keys(&pressed);
        let edge = |key: Keycode| keys.contains(&key) && !prev.contains(&key);
        if (edge(Keycode::LShift) || edge(Keycode::RShift)) && shift_guard == 0 {
            let open = if panel {
                !panel::is_open()
            } else {
                !OPEN.load(Ordering::SeqCst)
            };
            if panel {
                panel::set_open(open);
            }
            OPEN.store(open, Ordering::SeqCst);
            shift_guard = REPEAT_GUARD;
        }
        shift_guard = shift_guard.saturating_sub(1);
        if edge(Keycode::Escape) && (panel::is_open() || OPEN.load(Ordering::Relaxed)) {
            if panel {
                panel::set_open(false);
            }
            OPEN.store(false, Ordering::SeqCst);
        }
        if loader::take_gui_toggle() {
            if panel {
                panel::set_open(true);
            }
            OPEN.store(true, Ordering::SeqCst);
        }
        if OPEN.load(Ordering::Relaxed) {
            let count = if panel {
                panel::count()
            } else {
                loader::mod_names().len()
            };
            if count > 0 {
                let mut cursor = CURSOR.load(Ordering::SeqCst);
                if edge(Keycode::Down) {
                    cursor = cursor.wrapping_add(1);
                }
                if edge(Keycode::Up) {
                    cursor = cursor.wrapping_sub(1);
                }
                cursor %= count;
                if edge(Keycode::Enter) && enter_guard == 0 {
                    if panel {
                        panel::toggle_index(cursor);
                    } else {
                        let names = loader::mod_names();
                        let name = &names[cursor];
                        let enabled = loader::mod_enabled(name);
                        loader::set_mod_enabled(name, !enabled);
                    }
                    enter_guard = REPEAT_GUARD;
                }
                CURSOR.store(cursor, Ordering::SeqCst);
            }
        }
        enter_guard = enter_guard.saturating_sub(1);
        prev = keys;
    }
}
