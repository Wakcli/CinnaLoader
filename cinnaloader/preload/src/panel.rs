//! The ClickGUI: the loader's mod list published as the client's own mod panel.
//!
//! The client renders personal panels natively (`mod_ui::ModUi`) with toggle, slider,
//! button, keybind and choice controls, and an open panel owns the pointer and the
//! keyboard. Publishing one therefore gives a real in-frame GUI with no client change.
//!
//! The types below mirror `crates/ui/src/mod_panel.rs` field for field, so the values we
//! hand over have the layout the client expects.
//!
//! Two things are worth knowing before changing this:
//!
//! * The client's own per-frame publish calls `set_mod_panel_open(false)` whenever no
//!   guest owns a panel, which closes ours again. We publish under an owner of our own
//!   (`set_mod_panel(_, None)` only clears the panel whose owner matches) and hook
//!   `set_mod_panel_open` to force its `open` byte to the menu's state.
//! * **Mouse clicks are not wired yet.** The client reports them through
//!   `ModUi::mod_panel_events`, whose returned `Vec<Event>` is only visible if we hook it
//!   and call the relocated original ourselves (`hook_closure_retn`); in this binary that
//!   call corrupts the client and takes it down, so the panel is driven by the keyboard
//!   instead. See `cinnaloader-notes/STATUS.md`.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use cinnaloader_loader as loader;

use cinnaloader_native::detour::{hook_closure_jmp_back, CallbackOption, HookFlags, Registers};
use cinnaloader_native::symbols::Symbols;

/// Mirrors `ui::mod_panel`; field order is the ABI.
mod abi {
    use std::collections::BTreeMap;

    #[derive(Clone, Copy, Default, PartialEq)]
    pub enum Theme {
        #[default]
        Default,
        Monochrome,
    }

    #[derive(Clone, Copy, Default, PartialEq)]
    pub enum Style {
        #[default]
        Standard,
        Compact,
    }

    #[derive(Clone, Copy, Default, PartialEq)]
    pub enum Icon {
        #[default]
        None,
        Pointer,
        Crosshair,
        Ruler,
        Settings,
    }

    pub enum SurfaceValue {
        Bool(bool),
        Number(f64),
        Text(String),
        Vector(Vec<f64>),
    }

    pub struct Surface {
        pub screen: String,
        pub document: String,
        pub bindings: BTreeMap<String, SurfaceValue>,
    }

    pub struct Section {
        pub icon: Icon,
        pub id: String,
        pub label: String,
        pub category: String,
        pub toggle: Option<String>,
        pub controls: Vec<String>,
    }

    pub enum Control {
        Toggle {
            id: String,
            label: String,
            value: bool,
        },
        Slider {
            id: String,
            label: String,
            value: f32,
            min: f32,
            max: f32,
            step: f32,
        },
        Button {
            id: String,
            label: String,
        },
        Keybind {
            id: String,
            label: String,
            key: String,
            capturing: bool,
        },
        Choice {
            id: String,
            label: String,
            index: u32,
            options: Vec<String>,
        },
    }

    pub struct Panel {
        pub surface: Option<Surface>,
        pub reference_size: Option<[f32; 2]>,
        pub theme: Theme,
        pub style: Style,
        pub title: String,
        pub toggle_key: String,
        pub dark: bool,
        pub controls: Vec<Control>,
        pub capture_key: bool,
        pub motion: bool,
        pub sections: Vec<Section>,
    }

    #[derive(Clone, Copy, PartialEq)]
    pub enum PanelUiOwner {
        Environment(usize),
        Registration,
    }
}

use abi::{Control, Icon, Panel, PanelUiOwner, Section, Theme};

/// The owner our panel is published under. Distinct from every client-side owner
/// (`Environment(host index)` / `Registration`) so the client's own publish does not
/// clear it.
const OWNER: PanelUiOwner = PanelUiOwner::Environment(usize::MAX);

/// `unsafe extern "sysv64" fn(&mut UiPresentationRuntime) -> &mut ModUi`.
type ModUiMut = unsafe extern "sysv64" fn(*mut u8) -> *mut u8;
/// `fn(&mut ModUi, PanelUiOwner, Option<&Panel>) -> Result<(), String>` (sret).
/// The owner type has no `repr`, so the C type check cannot express it; the register
/// layout still comes from the client's own definition.
#[allow(improper_ctypes_definitions)]
type SetModPanel = unsafe extern "sysv64" fn(*mut u8, *mut u8, PanelUiOwner, *const Panel);
/// `fn(&mut ModUi, bool)`.
type SetModPanelOpen = unsafe extern "sysv64" fn(*mut u8, bool);

static MOD_UI_MUT: AtomicUsize = AtomicUsize::new(0);
static SET_MOD_PANEL: AtomicUsize = AtomicUsize::new(0);
static SET_MOD_PANEL_OPEN: AtomicUsize = AtomicUsize::new(0);
/// The menu is showing; the open hook forces the client's flag to match.
static OPEN: AtomicBool = AtomicBool::new(false);
/// Whether a panel has been published at least once.
static PUBLISHED: AtomicBool = AtomicBool::new(false);

/// Resolves the client's panel entry points and installs the open hook.
pub fn install(syms: &Symbols, base: usize) -> bool {
    let resolve = |leaf: &str| -> Option<usize> {
        syms.find(leaf)
            .into_iter()
            .filter(|(name, _)| name.rsplit("::").next() == Some(leaf))
            .map(|(_, address)| base + address as usize)
            .min()
    };
    let (Some(mod_ui_mut), Some(set_panel), Some(set_open)) = (
        resolve("mod_ui_mut"),
        resolve("set_mod_panel"),
        resolve("set_mod_panel_open"),
    ) else {
        eprintln!("[cinnaloader] panel: the client has no mod panel API (built without local-mods)");
        return false;
    };
    MOD_UI_MUT.store(mod_ui_mut, Ordering::SeqCst);
    SET_MOD_PANEL.store(set_panel, Ordering::SeqCst);
    SET_MOD_PANEL_OPEN.store(set_open, Ordering::SeqCst);

    // A closure that captures part of its environment: ilhook hands the closure's
    // address to the trampoline, and a zero-sized closure leaves it dangling.
    let entry = set_open != 0;
    let hook = unsafe {
        hook_closure_jmp_back(
            set_open,
            move |regs: *mut Registers| {
                if !entry {
                    return;
                }
                if PUBLISHED.load(Ordering::Relaxed) {
                    // `open` is the first argument after `&mut self`: sil.
                    (*regs).rsi = u64::from(OPEN.load(Ordering::Relaxed));
                }
            },
            CallbackOption::None,
            HookFlags::empty(),
        )
    };
    match hook {
        Ok(hook) => {
            std::mem::forget(hook);
            eprintln!("[cinnaloader] panel: hooked set_mod_panel_open");
        }
        Err(error) => {
            eprintln!("[cinnaloader] panel: open hook failed: {error:?}");
            return false;
        }
    }
    true
}

/// Shows or hides the menu.
pub fn set_open(open: bool) {
    OPEN.store(open, Ordering::SeqCst);
}

#[must_use]
pub fn is_open() -> bool {
    OPEN.load(Ordering::SeqCst)
}

/// The modules shown in the menu, in panel order, so the keyboard can address them.
static ORDER: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// One selectable row of the menu.
enum Entry {
    /// A setting a mod registered, by scoped id.
    Setting(String),
    /// A whole mod, by name, as an enable/disable row.
    Mod(String),
}

/// The client control that draws one setting.
fn control(setting: &loader::Setting, id: String) -> Control {
    use loader::SettingKind;
    match setting.kind {
        SettingKind::Toggle => Control::Toggle {
            id,
            label: setting.label.clone(),
            value: setting.on,
        },
        SettingKind::Slider => Control::Slider {
            id,
            label: setting.label.clone(),
            value: setting.number as f32,
            min: setting.min as f32,
            max: setting.max as f32,
            step: setting.step as f32,
        },
        SettingKind::Choice => Control::Choice {
            id,
            label: setting.label.clone(),
            index: setting.index,
            options: setting.options.clone(),
        },
        SettingKind::Keybind => Control::Keybind {
            id,
            label: setting.label.clone(),
            key: setting.key.clone(),
            capturing: false,
        },
        SettingKind::Button => Control::Button {
            id,
            label: setting.label.clone(),
        },
    }
}

/// A control id the client accepts, from a scoped feature id.
fn control_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_-.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect()
}

/// The mod list as the client's panel: one `Section` per category, one `Toggle` per
/// feature. Republished every frame on purpose: the client flips its own copy of a
/// control when the pointer presses it, and until clicks are harvested (see the module
/// comment) that copy is not ours to trust.
pub fn sync(presentation: *mut u8, settings: &[loader::Setting]) {
    let (Some(mod_ui_mut), Some(set_panel)) = (
        func::<ModUiMut>(MOD_UI_MUT.load(Ordering::SeqCst)),
        func::<SetModPanel>(SET_MOD_PANEL.load(Ordering::SeqCst)),
    ) else {
        return;
    };
    let mod_ui = unsafe { mod_ui_mut(presentation) };
    if mod_ui.is_null() {
        return;
    }

    let mut controls: Vec<Control> = Vec::with_capacity(settings.len());
    let mut sections: Vec<Section> = Vec::new();
    let mut order: Vec<Entry> = Vec::with_capacity(settings.len());
    let mut seen: Vec<(String, String)> = Vec::new();
    for setting in settings {
        let id = unique_control_id(&control_id(&setting.id), &seen);
        seen.push((id.clone(), setting.id.clone()));
        order.push(Entry::Setting(setting.id.clone()));
        controls.push(control(setting, id.clone()));
        push_control(&mut sections, &setting.category, id);
    }
    for name in loader::mod_names() {
        let id = unique_control_id(&control_id(&format!("mod_{name}")), &seen);
        seen.push((id.clone(), name.clone()));
        let enabled = loader::mod_enabled(&name);
        order.push(Entry::Mod(name.clone()));
        controls.push(Control::Toggle {
            id: id.clone(),
            label: name,
            value: enabled,
        });
        push_control(&mut sections, "Mods", id);
    }
    if let Ok(mut current) = ORDER.lock() {
        *current = order;
    }
    if let Ok(mut published) = CONTROL_IDS.lock() {
        *published = seen;
    }

    let panel = Panel {
        surface: None,
        reference_size: None,
        theme: Theme::Default,
        style: abi::Style::Standard,
        title: "cinnaloader".to_owned(),
        toggle_key: "ShiftRight".to_owned(),
        dark: true,
        controls,
        capture_key: false,
        motion: false,
        sections,
    };
    let mut result = [0u8; 32];
    unsafe { set_panel(&mut result as *mut _ as *mut u8, mod_ui, OWNER, &panel) };
    PUBLISHED.store(true, Ordering::SeqCst);
    if let Some(set_open) = func::<SetModPanelOpen>(SET_MOD_PANEL_OPEN.load(Ordering::SeqCst)) {
        unsafe { set_open(mod_ui, OPEN.load(Ordering::SeqCst)) };
    }
}

/// Panel control id → scoped feature id, for the ids the client reports on a click.
static CONTROL_IDS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// A control id the client accepts: bounded to `[A-Za-z0-9_\-.]` and unique.
fn unique_control_id(id: &str, taken: &[(String, String)]) -> String {
    let mut id = if id.is_empty() {
        "feature".to_owned()
    } else {
        id.to_owned()
    };
    let mut suffix = 1;
    while taken.iter().any(|(used, _)| used == &id) {
        suffix += 1;
        id.truncate(45);
        id.push_str(&format!("_{suffix}"));
    }
    id
}

/// Adds a control to its category's tab, creating the tab on first use.
fn push_control(sections: &mut Vec<Section>, category: &str, control: String) {
    match sections.iter_mut().find(|s| s.label == category) {
        Some(section) => section.controls.push(control),
        None => sections.push(Section {
            icon: Icon::None,
            id: control_id(category),
            label: category.to_owned(),
            category: category.to_owned(),
            toggle: None,
            controls: vec![control],
        }),
    }
}

/// How many rows the menu shows, in panel order.
#[must_use]
pub fn count() -> usize {
    ORDER.lock().map(|order| order.len()).unwrap_or(0)
}

/// Toggles the row at `index` in panel order.
pub fn toggle_index(index: usize) {
    let Ok(order) = ORDER.lock() else {
        return;
    };
    if order.is_empty() {
        return;
    }
    match &order[index % order.len()] {
        Entry::Setting(id) => {
            let id = id.clone();
            drop(order);
            loader::press_setting(&id);
        }
        Entry::Mod(name) => {
            let name = name.clone();
            drop(order);
            let enabled = loader::mod_enabled(&name);
            loader::set_mod_enabled(&name, !enabled);
        }
    }
}

/// One of the client's entry points, or `None` before installation.
fn func<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        return None;
    }
    Some(unsafe { std::mem::transmute_copy::<usize, T>(&address) })
}
