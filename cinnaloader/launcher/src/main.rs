//! Cinnaloader launcher: a standalone front end to the loader, separate from any client.
//!
//! `launch` runs a target process with the preload runtime attached, so the mods are
//! loaded inside it at startup. `run` and `list` exercise the loader directly.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use cinnaloader_loader::Host;
use cinnaloader_native::symbols::Symbols;

#[path = "../../preload/src/probe.rs"]
mod probe;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    match args.first().map(String::as_str) {
        Some("list") => {
            let dir = dir_arg(args.get(1))?;
            let host = Host::load_dir(&dir).map_err(|error| format!("{error:#}"))?;
            for name in host.names() {
                println!("{name}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("run") => {
            let dir = dir_arg(args.get(1))?;
            let mut host = Host::load_dir(&dir).map_err(|error| format!("{error:#}"))?;
            println!("{} mod(s) loaded from {}", host.len(), dir.display());
            for name in host.names() {
                println!("  {name}");
            }
            // A mod that schedules work for later needs ticks and real time to get there, so
            // both are a caller's choice: `run <dir> [ticks] [millis per tick]`. Without a
            // pacing, the ticks run as fast as they can, which is what a short check wants.
            let ticks: u32 = args
                .get(2)
                .and_then(|ticks| ticks.parse().ok())
                .unwrap_or(125);
            let pace: u64 = args
                .get(3)
                .and_then(|pace| pace.parse().ok())
                .unwrap_or(0);
            for _ in 0..ticks {
                host.tick(16);
                if pace > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(pace));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Some("overlay") | Some("clickgui") => {
            let dir = dir_arg(args.get(1))?;
            overlay(&dir)
        }
        Some("probe") => probe(args.get(1)),
        Some("launch") => {
            let client = args.get(1).ok_or("launch needs a client executable path")?;
            let dir = parse_mods_flag(args)?.unwrap_or_else(|| PathBuf::from("cinnaloader"));
            launch(Path::new(client), &dir)
        }
        Some("help") | None => {
            print_help();
            Ok(ExitCode::SUCCESS)
        }
        Some(other) => Err(format!(
            "unknown command `{other}` (try `cinnaloader help`)"
        )),
    }
}

/// Parses an ELF's symbols and prints which capability suffixes it contains. Called
/// with the running binary when no path is given.
fn probe(path: Option<&String>) -> Result<ExitCode, String> {
    let path = path.map_or_else(|| PathBuf::from("/proc/self/exe"), PathBuf::from);
    let Ok(symbols) = Symbols::from_path(&path) else {
        println!("usage: cinnaloader probe [BINARY]   (default: /proc/self/exe)");
        return Ok(ExitCode::SUCCESS);
    };
    print!("{}", probe::format(&probe::from_symbols(&symbols)));
    Ok(ExitCode::SUCCESS)
}

fn dir_arg(value: Option<&String>) -> Result<PathBuf, String> {
    value
        .map(PathBuf::from)
        .ok_or_else(|| "this command needs a mods directory".to_owned())
}

fn parse_mods_flag(args: &[String]) -> Result<Option<PathBuf>, String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--mods" {
            let value = iter.next().ok_or("--mods needs a value")?;
            return Ok(Some(PathBuf::from(value)));
        }
    }
    Ok(None)
}

/// The preload runtime beside this launcher, or `CINNALOADER_PRELOAD` when set.
fn preload_path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("CINNALOADER_PRELOAD") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let dir = exe.parent().ok_or("launcher has no parent directory")?;
    let name = if cfg!(target_os = "macos") {
        "libcinnaloader_preload.dylib"
    } else if cfg!(target_os = "windows") {
        "cinnaloader_preload.dll"
    } else {
        "libcinnaloader_preload.so"
    };
    let path = dir.join(name);
    if !path.is_file() {
        return Err(format!(
            "preload runtime not found at {} (set CINNALOADER_PRELOAD)",
            path.display()
        ));
    }
    Ok(path)
}

#[cfg(target_os = "linux")]
fn launch(client: &Path, mods: &Path) -> Result<ExitCode, String> {
    use std::os::unix::process::CommandExt;
    let preload = preload_path()?;
    let existing = std::env::var("LD_PRELOAD").unwrap_or_default();
    let ld_preload = if existing.is_empty() {
        preload.display().to_string()
    } else {
        format!("{}:{existing}", preload.display())
    };
    eprintln!(
        "[cinnaloader] launching {} with mods from {}",
        client.display(),
        mods.display()
    );
    let error = std::process::Command::new(client)
        .env("LD_PRELOAD", ld_preload)
        .env(cinnaloader_preload_mods_env(), mods)
        .exec();
    Err(format!("could not launch {}: {error}", client.display()))
}

#[cfg(not(target_os = "linux"))]
fn launch(client: &Path, _mods: &Path) -> Result<ExitCode, String> {
    Err(format!(
        "launch via LD_PRELOAD is Linux-only; on this platform add the preload runtime \
         to {} manually",
        client.display()
    ))
}

fn cinnaloader_preload_mods_env() -> &'static str {
    "CINNALOADER_DIR"
}

/// Loads the mods and shows the ClickGUI: an always-on-top window of clickable mod
/// toggles, toggled by the `RShift` keybind or by a mod's `toggle_gui()` call.
/// Loads the mods and shows the shared ClickGUI (the same view the in-client runtime shows).
fn overlay(dir: &Path) -> Result<ExitCode, String> {
    let host = Host::load_dir(dir).map_err(|error| format!("{error:#}"))?;
    cinnaloader_loader::gui::run(Some(host), true)?;
    Ok(ExitCode::SUCCESS)
}

fn print_help() {
    println!(
        "\
cinnaloader — standalone mod loader for a target process

USAGE:
  cinnaloader list <MODS_DIR>              List mods the loader accepts
  cinnaloader run <MODS_DIR>               Load mods and tick them (self-test)
  cinnaloader overlay <MODS_DIR>           Visible always-on-top overlay of mod output
  cinnaloader clickgui <MODS_DIR>          ClickGUI (mod toggles) toggled by RShift
  cinnaloader launch <CLIENT> [--mods DIR] Run the client with the loader attached
  cinnaloader probe [BINARY]               Show which client symbols each capability resolves
  cinnaloader help

ENVIRONMENT:
  CINNALOADER_DIR        Mods directory used at launch (default: ./cinnaloader)
  CINNALOADER_PRELOAD    Path to the preload runtime (default: beside this binary)"
    );
}
