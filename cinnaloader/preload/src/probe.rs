//! Which client symbols each planned capability actually resolves against.
//!
//! The client's small helpers are inlined away in release builds, so a capability is
//! only real when every symbol it needs survives in the running binary's `.symtab`.
//! The loader reads this table once at startup and a regression compares against it.
//!
//! The suffixes below were derived by disassembling the client. Four of them name
//! legacy-mangled fragments (`modding9drive_mod` and friends) that never appear in a
//! demangled name, so they are written here in their demangled form instead.

#![allow(dead_code)]

use cinnaloader_native::symbols::{self, Symbols};

/// One planned capability and the demangled symbol suffixes it depends on.
pub struct Capability {
    pub name: &'static str,
    pub symbols: &'static [&'static str],
}

/// The capability-truth table, in the order the report lists it.
pub const CAPABILITIES: &[Capability] = &[
    Capability {
        name: "live_frame_reads",
        symbols: &["ModHost>::frame_with_movement", "Instance>::frame"],
    },
    Capability {
        name: "gameplay_context",
        symbols: &["modding::drive_mod", "modding::multi::run_frame"],
    },
    Capability {
        name: "pose_read",
        symbols: &[
            "LocalViewPose>::eye_translation",
            "LocalViewPose>::feet_translation",
            "LocalViewPose>::rotation",
        ],
    },
    Capability {
        name: "aim_rotate",
        symbols: &["LocalViewPose>::set_rotation"],
    },
    Capability {
        name: "actors_read",
        symbols: &[
            "WorldAuthority>::actor_pose_by_unique",
            "WorldAuthority>::actor_health_by_unique",
            "WorldAuthority>::canonical_item_stack",
            "WorldAuthority>::block_ids_for_name",
        ],
    },
    Capability {
        name: "clicker_attack",
        symbols: &[
            "MeleeRuntime>::observe_input",
            "melee::produce_melee",
            "FinalizedInput>::request_mod_attack_press",
        ],
    },
    Capability {
        name: "jump_reset",
        symbols: &[
            "LocalPhysicsController>::request_jump_pulse",
            "LocalPhysicsController>::set_jump_pulse_scope",
            "LocalPhysicsController>::state",
        ],
    },
    Capability {
        name: "block_highlights",
        symbols: &["ModRenderScene>::set_block_highlights"],
    },
    Capability {
        name: "fullbright",
        symbols: &["modding::fullbright", "WorldFullbright"],
    },
    Capability {
        name: "time_override",
        symbols: &["VisualTimeOverride", "update_atmosphere_frame"],
    },
    Capability {
        name: "panel_surface",
        symbols: &[
            "capture_hud_frame",
            "UiPresentationRuntime>::swap_debug_lines",
            "ModUi>::set_mod_panel",
            "ModUi>::set_mod_panel_open",
        ],
    },
];

/// One `(capability, symbol)` probe result, with the matched link-time address when
/// the symbol is present.
pub struct Row {
    pub capability: &'static str,
    pub symbol: &'static str,
    pub found: Option<usize>,
}

/// Resolves every capability suffix against a set of demangled `(name, address)`
/// pairs. Taking pairs rather than a [`Symbols`] keeps the table testable without an
/// ELF and usable from the CLI, which parses a file that is not the running one.
#[must_use]
pub fn rows<'a>(symbols: impl IntoIterator<Item = (&'a str, u64)>) -> Vec<Row> {
    let symbols: Vec<(&str, u64)> = symbols.into_iter().collect();
    let mut rows = Vec::new();
    for capability in CAPABILITIES {
        for symbol in capability.symbols {
            let found = symbols
                .iter()
                .find(|(name, _)| name.contains(symbol))
                .map(|(_, address)| usize::try_from(*address).unwrap_or(usize::MAX));
            rows.push(Row {
                capability: capability.name,
                symbol,
                found,
            });
        }
    }
    rows
}

/// [`rows`] over a parsed symbol table.
#[must_use]
pub fn from_symbols(symbols: &Symbols) -> Vec<Row> {
    rows(symbols.find(""))
}

/// The stable, greppable table: one detail line per `(capability, symbol)`, then one
/// `tier` summary per capability.
#[must_use]
pub fn format(rows: &[Row]) -> String {
    let mut out = String::new();
    for row in rows {
        let state = if row.found.is_some() { "found" } else { "missing" };
        out.push_str(&format!("probe\t{}\t{}\t{}\n", row.capability, state, row.symbol));
    }
    for capability in CAPABILITIES {
        let found = rows
            .iter()
            .filter(|row| row.capability == capability.name && row.found.is_some())
            .count();
        let total = capability.symbols.len();
        out.push_str(&format!(
            "probe\t{}\t{}\t{found}/{total}\n",
            capability.name,
            tier(found, total)
        ));
    }
    out
}

/// Resolves the table against the running client and logs it, one capability per line.
pub fn report() {
    let needles: Vec<&str> = CAPABILITIES
        .iter()
        .flat_map(|capability| capability.symbols.iter().copied())
        .map(last_segment)
        .collect();
    let syms = match symbols::current_exe_symbols_matching(&needles) {
        Ok(syms) => syms,
        Err(error) => {
            eprintln!("[cinnaloader] probe: cannot read the client symbol table: {error:#}");
            return;
        }
    };
    for line in format(&from_symbols(&syms)).lines() {
        eprintln!("[cinnaloader] {line}");
    }
}

fn tier(found: usize, total: usize) -> &'static str {
    if found == 0 {
        "none"
    } else if found == total {
        "full"
    } else {
        "partial"
    }
}

/// The final path segment of a demangled suffix, which is the substring that survives
/// mangling and can pre-filter the expensive parse.
fn last_segment(symbol: &str) -> &str {
    symbol.rsplit("::").next().unwrap_or(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn some_symbols_present_is_partial() {
        let rows = rows([("cinnabar::x::<client::ModHost>::frame_with_movement", 0x1000u64)]);
        let text = format(&rows);
        assert!(
            text.contains("probe\tlive_frame_reads\tfound\tModHost>::frame_with_movement"),
            "{text}"
        );
        assert!(
            text.contains("probe\tlive_frame_reads\tmissing\tInstance>::frame"),
            "{text}"
        );
        assert!(text.contains("probe\tlive_frame_reads\tpartial\t1/2"), "{text}");
    }

    #[test]
    fn no_symbols_present_is_none() {
        let rows = rows([("totally::unrelated_symbol", 0x1u64)]);
        let text = format(&rows);
        assert!(text.contains("probe\tfullbright\tnone\t0/2"), "{text}");
        assert!(text.contains("probe\tpanel_surface\tnone\t0/4"), "{text}");
        assert!(!text.contains("\tfull\t"), "{text}");
        assert!(!text.contains("\tpartial\t"), "{text}");
    }

    #[test]
    fn all_symbols_present_is_full() {
        let rows = rows([
            ("<client::ModHost>::frame_with_movement", 0x1u64),
            ("<client::Instance>::frame", 0x2u64),
        ]);
        let text = format(&rows);
        assert!(text.contains("probe\tlive_frame_reads\tfull\t2/2"), "{text}");
    }
}
