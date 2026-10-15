//! Resolves symbols from the process's own executable.
//!
//! The loader runs *inside* the client, so `/proc/self/exe` is the client binary.
//! Its `.symtab` (kept by the packaged build) names every function; combined with the
//! runtime load base from `/proc/self/maps`, a name becomes a live address.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use object::{Object, ObjectSymbol};

/// Name-to-virtual-address table parsed from one ELF object.
pub struct Symbols {
    map: HashMap<String, u64>,
}

impl Symbols {
    /// Parses every defined symbol, demangling Rust names to their readable form.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let file = object::File::parse(data).context("parsing the object file")?;
        let mut map = HashMap::new();
        for symbol in file.symbols() {
            if !symbol.is_definition() {
                continue;
            }
            let Ok(name) = symbol.name() else {
                continue;
            };
            let readable = rustc_demangle::try_demangle(name)
                .map(|demangled| format!("{demangled:#}"))
                .unwrap_or_else(|_| name.to_owned());
            map.entry(readable).or_insert(symbol.address());
        }
        Ok(Self { map })
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&data)
    }

    /// Parses only the symbols whose mangled name contains one of `needles`.
    ///
    /// Demangling every name is most of [`Symbols::parse`]'s cost, and this runs before
    /// the target's `main`, so a caller that knows the few names it wants pays almost
    /// none of it.
    pub fn parse_matching(data: &[u8], needles: &[&str]) -> Result<Self> {
        let file = object::File::parse(data).context("parsing the object file")?;
        let mut map = HashMap::new();
        for symbol in file.symbols() {
            if !symbol.is_definition() {
                continue;
            }
            let Ok(name) = symbol.name() else {
                continue;
            };
            if !needles.iter().any(|needle| name.contains(needle)) {
                continue;
            }
            let readable = rustc_demangle::try_demangle(name)
                .map(|demangled| format!("{demangled:#}"))
                .unwrap_or_else(|_| name.to_owned());
            map.entry(readable).or_insert(symbol.address());
        }
        Ok(Self { map })
    }

    /// The symbol's link-time address (offset within the image).
    #[must_use]
    pub fn offset(&self, name: &str) -> Option<u64> {
        self.map.get(name).copied()
    }

    /// Every name containing `needle`, for discovery and version maps.
    #[must_use]
    pub fn find(&self, needle: &str) -> Vec<(&str, u64)> {
        self.map
            .iter()
            .filter(|(name, _)| name.contains(needle))
            .map(|(name, address)| (name.as_str(), *address))
            .collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Symbols of the executable this code is running inside.
pub fn current_exe_symbols() -> Result<Symbols> {
    Symbols::from_path(&current_exe_path()?)
}

/// The named symbols of the executable this code is running inside, by substring of
/// their mangled name. Much cheaper than [`current_exe_symbols`] when only a few names
/// are wanted, which matters when the caller hooks before the target starts.
pub fn current_exe_symbols_matching(needles: &[&str]) -> Result<Symbols> {
    let path = current_exe_path()?;
    let data = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    Symbols::parse_matching(&data, needles)
}

/// `/proc/self/exe` — the real binary, even after `LD_PRELOAD` relocation.
pub fn current_exe_path() -> Result<PathBuf> {
    std::fs::read_link("/proc/self/exe").context("reading /proc/self/exe")
}

/// The load base of the main executable: the start of the mapping at file offset 0.
pub fn load_base() -> Result<usize> {
    let exe = current_exe_path()?;
    load_base_for(&exe)
}

/// The load base reported by `/proc/self/maps` for `exe`, when this process maps it.
pub fn load_base_for(exe: &Path) -> Result<usize> {
    let maps = std::fs::read_to_string("/proc/self/maps").context("reading /proc/self/maps")?;
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(range), Some(_perms), Some(offset)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if offset != "00000000" {
            continue;
        }
        let path = fields.nth(2).unwrap_or("");
        if Path::new(path) != exe {
            continue;
        }
        let start = range
            .split_once('-')
            .and_then(|(start, _)| usize::from_str_radix(start, 16).ok())
            .context("malformed maps range")?;
        return Ok(start);
    }
    anyhow::bail!("no offset-0 mapping for {}", exe.display())
}

/// Resolves a live address for `name` in the current process, or `None` if absent.
pub fn resolve_current(name: &str) -> Result<Option<usize>> {
    let symbols = current_exe_symbols()?;
    let Some(offset) = symbols.offset(name) else {
        return Ok(None);
    };
    Ok(Some(load_base()? + offset as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[no_mangle]
    pub extern "C" fn cinnaloader_native_probe() -> u32 {
        7
    }

    #[test]
    fn resolves_an_exported_symbol_of_this_process_to_its_live_address() {
        let symbols = current_exe_symbols().expect("read own symbols");
        let offset = symbols
            .offset("cinnaloader_native_probe")
            .expect("probe symbol present");
        let base = load_base().expect("load base");
        let live = base + offset as usize;
        let probe: extern "C" fn() -> u32 = cinnaloader_native_probe;
        let actual = probe as usize;
        assert_eq!(live, actual, "resolved address must match the symbol");
    }

    #[test]
    fn finds_a_client_function_in_the_installed_binary() {
        let path = std::env::var_os("CINNALOADER_CLIENT_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    "/home/wak/.local/share/cinnabar/app/v0.1.19-799580031ec0/usr/bin/cinnabar",
                )
            });
        if !path.is_file() {
            eprintln!("skipping: no client binary at {}", path.display());
            return;
        }
        let symbols = Symbols::from_path(&path).expect("parse client");
        assert!(
            symbols.len() > 100_000,
            "expected a large symbol table, got {}",
            symbols.len()
        );
        let hits = symbols.find("queue_chat_send");
        assert!(
            !hits.is_empty(),
            "queue_chat_send not found in the client symbol table"
        );
        eprintln!("found {} symbol(s) for queue_chat_send", hits.len());
    }

    #[test]
    fn a_targeted_lookup_matches_the_full_parse() {
        let path = std::env::var_os("CINNALOADER_CLIENT_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(
                    "/home/wak/.local/share/cinnabar/app/v0.1.19-799580031ec0/usr/bin/cinnabar",
                )
            });
        if !path.is_file() {
            eprintln!("skipping: no client binary at {}", path.display());
            return;
        }
        let data = std::fs::read(&path).expect("read client");
        let name =
            "<client_ui::ui_runtime::presentation::UiPresentationRuntime>::swap_debug_lines";
        let few = Symbols::parse_matching(&data, &["swap_debug_lines"]).expect("targeted parse");
        assert!(few.len() < 16, "targeted parse kept {} symbols", few.len());
        assert_eq!(few.offset(name), Symbols::parse(&data).unwrap().offset(name));
        assert!(few.offset(name).is_some());
    }
}
