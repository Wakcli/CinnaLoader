//! Live reads from the running Cinnabar client, published through the loader's `observe`
//! slot.
//!
//! Each value comes from calling a client accessor whose address was resolved from the
//! running binary's own symbol table, once, before the frame hook runs. A read whose
//! symbol did not resolve is left out of the note rather than guessed, so a key appears
//! only when its value actually came off the client.
//!
//! `capture_hud_frame` hands the hook the client's own registers: `rsi` is the player,
//! `rdx` is `&mut UiRuntime`, `rcx` is `&mut UiPresentationRuntime`, and `r8` is
//! `Option<&WorldStream>`, null when there is no stream. Only `rdx` and `r8` are read
//! here. The per-frame work is a handful of calls and one small string.

use std::sync::OnceLock;

use cinnaloader_native::detour::Registers;
use cinnaloader_native::symbols::{self, Symbols};

/// The demangled suffix of each client accessor the live reads call.
const SESSION: &str = "UiRuntime>::session_id";
const DIMENSION: &str = "WorldStream>::current_dimension";
const ALIVE: &str = "UiRuntime>::local_player_alive";
const HUD: &str = "UiRuntime>::hud";
const HEALTH: &str = "ui::hud::HudStore>::health";
const HUNGER: &str = "ui::hud::HudStore>::hunger";
const CURRENT: &str = "ui::hud::BoundedStat>::current";
const MAXIMUM: &str = "ui::hud::BoundedStat>::maximum";
const GAMEPLAY_HUD: &str = "UiRuntime>::gameplay_hud";
const SATURATION: &str = "GameplayHudState>::saturation";

/// The substrings that pre-filter the symbol parse to the accessors above.
const NEEDLES: [&str; 10] = [
    "session_id",
    "current_dimension",
    "local_player_alive",
    "hud",
    "health",
    "hunger",
    "current",
    "maximum",
    "gameplay_hud",
    "saturation",
];

/// Mirrors `ui::hud::BoundedStat`. All three fields are kept, and in this order, so the
/// type is six bytes and the client's `health`/`hunger` hand its `Option` back the way the
/// client built it (a `u16` discriminant followed by the payload); dropping `scale` would
/// change the size and the ABI.
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct BoundedStat {
    current: u16,
    maximum: u16,
    scale: u16,
}

// The client's own definitions have no `repr`, so the C type check cannot express them;
// the register layout still comes from the client's definition. Same as `panel.rs`.
type SessionId = unsafe extern "sysv64" fn(*const u8) -> u64;
type Dimension = unsafe extern "sysv64" fn(*const u8) -> i32;
#[allow(improper_ctypes_definitions)]
type Alive = unsafe extern "sysv64" fn(*const u8) -> Option<bool>;
type Object = unsafe extern "sysv64" fn(*const u8) -> *const u8;
#[allow(improper_ctypes_definitions)]
type Stat = unsafe extern "sysv64" fn(*const u8) -> Option<BoundedStat>;
#[allow(improper_ctypes_definitions)]
type StatPart = unsafe extern "sysv64" fn(BoundedStat) -> u16;
#[allow(improper_ctypes_definitions)]
type Saturation = unsafe extern "sysv64" fn(*const u8) -> Option<f32>;

/// The client accessors the live reads call, each present only when its symbol resolved.
#[derive(Default)]
struct Resolved {
    session: Option<SessionId>,
    dimension: Option<Dimension>,
    alive: Option<Alive>,
    hud: Option<Object>,
    health: Option<Stat>,
    hunger: Option<Stat>,
    current: Option<StatPart>,
    maximum: Option<StatPart>,
    gameplay_hud: Option<Object>,
    saturation: Option<Saturation>,
}

impl Resolved {
    /// How many of the live-read accessors resolved.
    fn count(&self) -> usize {
        [
            self.session.is_some(),
            self.dimension.is_some(),
            self.alive.is_some(),
            self.hud.is_some(),
            self.health.is_some(),
            self.hunger.is_some(),
            self.current.is_some(),
            self.maximum.is_some(),
            self.gameplay_hud.is_some(),
            self.saturation.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }
}

static RESOLVED: OnceLock<Resolved> = OnceLock::new();

/// Resolves the client accessors once, before any frame runs, and says how many it found.
/// Called from the menu's install, beside the other symbol lookups.
pub fn install() {
    let resolved = resolve();
    eprintln!(
        "[cinnaloader] live: {}/{} client reads resolved",
        resolved.count(),
        NEEDLES.len()
    );
    let _ = RESOLVED.set(resolved);
}

/// Resolves every live-read accessor from the running binary, leaving each `None` when its
/// symbol is absent.
fn resolve() -> Resolved {
    let Ok(syms) = symbols::current_exe_symbols_matching(&NEEDLES) else {
        return Resolved::default();
    };
    let Ok(base) = symbols::load_base() else {
        return Resolved::default();
    };
    let at = |suffix: &str| address(&syms, base, suffix);
    Resolved {
        session: at(SESSION).and_then(func),
        dimension: at(DIMENSION).and_then(func),
        alive: at(ALIVE).and_then(func),
        hud: at(HUD).and_then(func),
        health: at(HEALTH).and_then(func),
        hunger: at(HUNGER).and_then(func),
        current: at(CURRENT).and_then(func),
        maximum: at(MAXIMUM).and_then(func),
        gameplay_hud: at(GAMEPLAY_HUD).and_then(func),
        saturation: at(SATURATION).and_then(func),
    }
}

/// The live address of the one symbol whose demangled name contains `suffix`.
fn address(syms: &Symbols, base: usize, suffix: &str) -> Option<usize> {
    syms.find(suffix)
        .into_iter()
        .map(|(_, address)| base + address as usize)
        .min()
}

/// A client function at `address`, or `None` when the symbol was missing.
fn func<T: Copy>(address: usize) -> Option<T> {
    (address != 0).then(|| unsafe { std::mem::transmute_copy::<usize, T>(&address) })
}

/// The client facts one frame could read. A field is `None` when its symbol did not
/// resolve, or when the client itself had nothing to hand back. Health and hunger are
/// reported the way the client draws them, points divided by the stat's own scale.
#[derive(Default)]
pub struct Facts {
    pub session: Option<u64>,
    pub dimension: Option<i32>,
    pub alive: Option<bool>,
    pub hp: Option<(f32, f32)>,
    pub hunger: Option<f32>,
    pub saturation: Option<f32>,
}

/// Formats the sampled facts as `key<TAB>value` lines, one per present key, with no
/// trailing newline. An all-absent set formats to the empty string.
#[must_use]
pub fn lines(facts: &Facts) -> String {
    let mut out = String::new();
    if let Some(session) = facts.session {
        out.push_str(&format!("session\t{session}\n"));
    }
    if let Some(dimension) = facts.dimension {
        out.push_str(&format!("dimension\t{dimension}\n"));
    }
    if let Some(alive) = facts.alive {
        out.push_str(&format!("alive\t{alive}\n"));
    }
    if let Some((current, maximum)) = facts.hp {
        out.push_str(&format!("hp\t{current}\nhp_max\t{maximum}\n"));
    }
    if let Some(hunger) = facts.hunger {
        out.push_str(&format!("hunger\t{hunger}\n"));
    }
    if let Some(saturation) = facts.saturation {
        out.push_str(&format!("saturation\t{saturation}\n"));
    }
    let len = out.trim_end_matches('\n').len();
    out.truncate(len);
    out
}

/// Reads what it can of the running client from `capture_hud_frame`'s register file, as
/// `key<TAB>value` lines. Empty when no symbol resolved or nothing could be read.
#[must_use]
pub fn sample(regs: *mut Registers) -> String {
    let Some(resolved) = RESOLVED.get() else {
        return String::new();
    };
    let runtime = unsafe { (*regs).rdx as *const u8 };
    let stream = unsafe { (*regs).r8 as *const u8 };
    let mut facts = Facts::default();
    if !runtime.is_null() {
        facts.session = resolved.session.map(|f| unsafe { f(runtime) });
        facts.alive = resolved.alive.and_then(|f| unsafe { f(runtime) });
    }
    if !stream.is_null() {
        facts.dimension = resolved.dimension.map(|f| unsafe { f(stream) });
    }
    facts.hp = health_stat(resolved, runtime);
    facts.hunger = hunger_stat(resolved, runtime);
    facts.saturation = saturation_value(resolved, runtime);
    lines(&facts)
}

/// `runtime.hud().health()` as display `(current, maximum)`, or `None` when a symbol is
/// missing or the stat's scale is zero.
fn health_stat(resolved: &Resolved, runtime: *const u8) -> Option<(f32, f32)> {
    let store = store_of(resolved, runtime)?;
    let value = unsafe { resolved.health?(store) }?;
    let scale = f32::from(value.scale);
    if scale == 0.0 {
        return None;
    }
    let current = unsafe { resolved.current?(value) };
    let maximum = unsafe { resolved.maximum?(value) };
    Some((f32::from(current) / scale, f32::from(maximum) / scale))
}

/// `runtime.hud().hunger()` in display points, or `None` when a symbol is missing or the
/// stat's scale is zero.
fn hunger_stat(resolved: &Resolved, runtime: *const u8) -> Option<f32> {
    let store = store_of(resolved, runtime)?;
    let value = unsafe { resolved.hunger?(store) }?;
    let scale = f32::from(value.scale);
    if scale == 0.0 {
        return None;
    }
    let current = unsafe { resolved.current?(value) };
    Some(f32::from(current) / scale)
}

/// `runtime.gameplay_hud().saturation()`, or `None` when a symbol is missing.
fn saturation_value(resolved: &Resolved, runtime: *const u8) -> Option<f32> {
    if runtime.is_null() {
        return None;
    }
    let gameplay_hud = resolved.gameplay_hud?;
    let saturation = resolved.saturation?;
    let state = unsafe { gameplay_hud(runtime) };
    if state.is_null() {
        return None;
    }
    unsafe { saturation(state) }
}

/// `runtime.hud()`, or `None` when the runtime is null or `hud` did not resolve.
fn store_of(resolved: &Resolved, runtime: *const u8) -> Option<*const u8> {
    if runtime.is_null() {
        return None;
    }
    let hud = resolved.hud?;
    let store = unsafe { hud(runtime) };
    (!store.is_null()).then_some(store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_present_key_becomes_a_line() {
        let facts = Facts {
            session: Some(42),
            dimension: Some(-3),
            alive: Some(true),
            hp: Some((7.0, 20.0)),
            hunger: Some(6.0),
            saturation: Some(2.5),
        };
        assert_eq!(
            lines(&facts),
            "session\t42\ndimension\t-3\nalive\ttrue\nhp\t7\nhp_max\t20\nhunger\t6\nsaturation\t2.5"
        );
    }

    #[test]
    fn absent_keys_are_left_out() {
        let facts = Facts {
            session: Some(1),
            alive: Some(false),
            ..Facts::default()
        };
        let text = lines(&facts);
        assert_eq!(text, "session\t1\nalive\tfalse");
        for absent in ["dimension", "hp", "hp_max", "hunger", "saturation"] {
            assert!(!text.contains(absent), "{absent} leaked into {text:?}");
        }
    }

    #[test]
    fn an_empty_facts_set_formats_to_nothing() {
        assert_eq!(lines(&Facts::default()), "");
    }
}
