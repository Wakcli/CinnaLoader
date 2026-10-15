# cinnaloader

A native mod loader. Mods are Rust cdylibs (`.so` / `.dylib` / `.dll`) that the loader
loads into the target process and drives from a tick thread.

```
sdk/         cinnaloader — the API mods build against
loader/      loading, ticking, settings, events, per-mod config, the log and GUI services
native/      ELF symbol resolution and x86-64 inline hooks
preload/     LD_PRELOAD runtime: loads mods inside the target and draws the menu
launcher/    the `cinnaloader` command-line front end
examples/    hello-mod, a worked example using every setting kind
```

## Writing a mod

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
cinnaloader = { path = "../cinnaloader/sdk" }
```

```rust
use cinnaloader::prelude::*;

#[derive(Default)]
struct Sprint {
    beat: Timer,
}

impl Mod for Sprint {
    fn load(&mut self) {
        meta!("Sprint", env!("CARGO_PKG_VERSION"), "you");

        feature("sprint", "Sprint", "Movement", ON);
        slider("speed", "Sprint speed", "Movement", 1.2, 0.1..=5.0, 0.1);
        choice("mode", "Sprint mode", "Movement", &["Legit", "Blatant"], 0);
        keybind("panic", "Panic key", "Movement", "P");
        button("reset", "Reset settings", "Movement");

        self.beat = Timer::every(1000);
        if config("speed").is_none() {
            set_config("speed", "1.2");
        }
    }

    fn tick(&mut self, dt_millis: u32) {
        if enabled("sprint") && self.beat.ready(dt_millis) {
            info!("sprinting at {:.1}", number("speed"));
        }
    }

    fn toggle(&mut self, id: &str, on: bool) {
        info!("{id} is now {on}");
    }

    fn keybind(&mut self, id: &str, down: bool) {
        if id == "panic" && down {
            set_enabled("sprint", false);
        }
    }
}

export_mod!(Sprint);
```

Build it and drop the artifact into the mods directory. `examples/hello-mod` uses every
setting kind, a keybind, a button, a timer and a saved value.

### Lifecycle

| method | when |
| --- | --- |
| `load` | once, with the host services installed; register settings here |
| `tick(dt_millis)` | every loader tick, about 60/s |
| `toggle` / `slider` / `choice` / `keybind` / `pressed` | the user changed that setting, delivered just before the next `tick` |
| `unload` | when the loader drops the mod — reached by the CLI; the in-client runtime exits with the process |

Every method has a default, so a mod implements only what it uses.

### Settings

`feature`, `slider`, `choice`, `keybind` and `button` each add a control to the loader's
in-game menu, grouped under a tab named by the `category` you pass. Ids are scoped to your
mod, so two mods may both use `"sprint"`.

Read them back with `enabled(id)`, `number(id)`, `choice_index(id)` and `key(id)`; write
them with `set_enabled` and `set_number`. Every setting is saved to `<mod>/<name>.cfg`
next to the mods and reloaded on the next launch, and `config(key)` / `set_config(key,
value)` do the same for values of your own.

### Host services

| call | effect |
| --- | --- |
| `info!` / `warn!` / `error!` | one line through the loader's log |
| `overlay_line(text)` | one line on the loader's on-screen overlay |
| `mods_dir()` | the directory the mods were loaded from |
| `toggle_gui()` | opens or closes the in-game menu |
| `meta!(name, version, authors)` | reports the mod's identity in the log |
| `Timer::after(ms)` / `Timer::every(ms)` | a rate driven by `tick`, via `ready(dt)` |

### ABI

The loader and a mod share only [`HostApi`], a table of function pointers, and the four
`cinnaloader_*` entry points `export_mod!` generates. `ABI_VERSION` guards the pair: a mod
built against a different version is refused with a message rather than loaded. Nothing
else crosses the boundary, so a mod does not link the loader and does not need a shared
runtime.

## Running

```sh
cargo build
cargo test
target/debug/cinnaloader list   <MODS_DIR>   # what the loader accepts
target/debug/cinnaloader run    <MODS_DIR>   # load, tick, unload
target/debug/cinnaloader launch <CLIENT> --mods <MODS_DIR>   # LD_PRELOAD a target
```

Inside the target, `Shift` opens the menu and `Escape` closes it. The tabs are the setting
categories your mods declare. `Up`/`Down` move the highlighted setting and `Enter` presses
it: pointer clicks on the menu are not wired to settings yet, so the menu is driven from
the loader's own key loop. Keybinds work independently of the menu.
