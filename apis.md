# APIs to implement

The work queue, generated from `minecraft_mod_loader_api_wishlist.md` (107 sections,
~3300 named functions). **That file is the specification; this file is the status.**

> **Goal.** Treat the wishlist as the global API roadmap for the whole project: build a
> complete, production-quality mod loader implementing as much of it as possible, prioritising
> foundational systems before dependent ones, keeping APIs consistent and features genuinely
> functional rather than stubs. Work it progressively, keep this file's status honest, and
> preserve what already works. The full statement is in `AGENTS.md`.

## How a section is closed

1. Read the section in the wishlist and decide, per function, which of these it is:
   - **implementable** in this loader — a host call, a setting, an event, or a thin wrapper
     the SDK can provide over what the client already exposes;
   - **needs hooks** — depends on symbol work into the client (gameplay reads, packets,
     rendering, input beyond our key poll);
   - **not mappable** — no equivalent in this client, or it names a Java/JVM concept that
     has no Bedrock counterpart (classloaders, JVM scripting, data components as Forge
     defines them). Say why in the Notes column and move on.
2. Implement the whole section, adding the API to the SDK at
   `cinnaloader/cinnaloader/sdk`, the host side at `.../loader`, and a test in the crate
   that owns it.
3. `cargo build`, `cargo test`, `cargo clippy --all-targets`.
4. **Verify in-game** with `docs/testing-in-game.md`: a headless client, the mod built
   against the new API, the API exercised, and the capture/log from that run named in the
   row. A section that cannot be verified in-game says so and stays `wip`.
5. Only then set the row and move to the next section. Bump `ABI_VERSION` whenever the
   boundary changes, and say so in the row.

Status: `todo` · `wip` · `done` · `blocked` · `n/a`

`Items` is the count of call-shaped names the wishlist lists in that section — a size
guide, not a contract.

| # | Section | Items | Status | Evidence / notes |
| --- | --- | --- | --- | --- |
| 1 | Mod metadata and entry points | 37 | done | ABI v6, `822ea60`. `/tmp/opencode/s1-metadata.png` |
| 2 | Mod lifecycle | 59 | done | ABI v7, `870fe52`. `/tmp/opencode/s2-lifecycle.png` |
| 3 | Versioning and compatibility | 35 | done | `95dc61d`. `/tmp/opencode/s3-compat.png`. 4 names have no target, see below |
| 4 | Dependencies and mod load order | 46 | done | ABI v8. `/tmp/opencode/s4-deps.png` + the run log: order, refusal, errors |
| 5 | Capability system | 10 | done | `Capability` grew from 8 names to the wishlist's 38, with `supported()`/`why_unsupported()` saying honestly what a mod can be given here. Verified in a live client: `held=[Render, FilesystemRead, FilesystemWrite]`, `world_read=false`, `unsupported_why=needs hooks into the client's world and entity state` |
| 6 | Event system | 24 | done | Bus with phases, priorities, filters, once, cancel and stop. Fired by the loader's own phases, so an event means the phase really happened |
| 7 | Threading and task scheduling | 32 | done | Tick, delayed, repeating, render-phase, worker and pool tasks, with tokens, groups, metrics and thread checks. The rules are in the module docs |
| 8 | Logging and diagnostics | 38 | done | Levelled log with sinks, filters, history, structured fields and error capture; a report, a health check and a capability report |
| 9 | Configuration system | 42 | done | Typed values with a schema, constraints, migrations, watchers, transactions and import/export/backup. Saved in `<config_dir>/config.ini` |
| 10 | Mod permissions and sandboxing | 16 | done | The wishlist's 29 permissions with a policy that names what this host can enforce and what it cannot. A mod cannot raise its own trust, and the loader clamps what a manifest claims |
| 11 | Filesystem and storage | 47 | done | `fs::*` rooted in the mod's own directory (escapes refused) and a key/value `storage::*` beside its config |
| 12 | Resource and asset management | 24 | done | Packs by namespace with priorities, overrides, reload listeners and a per-mod `assets/` pack |
| 13 | Localization | 21 | done | Keys per language with a fallback, `one\|many` plurals, placeholders, and the handful of formats a player sees |
| 14 | Game and session state | 33 | todo |
| 15 | World and dimension APIs | 56 | todo |
| 16 | Block APIs | 50 | todo |
| 17 | Item APIs | 47 | todo |
| 18 | Inventory and containers | 50 | todo |
| 19 | Entity system | 82 | todo |
| 20 | Entity components and attributes | 27 | todo |
| 21 | Player APIs | 65 | todo |
| 22 | Status effects and damage | 29 | todo |
| 23 | Physics and collision | 35 | todo |
| 24 | Raycasting and interaction | 32 | todo |
| 25 | Movement and locomotion | 36 | todo |
| 26 | World generation | 35 | todo |
| 27 | Recipes, crafting, and smelting | 29 | todo |
| 28 | Registries | 25 | todo |
| 29 | Rendering lifecycle and render hooks | 38 | todo |
| 30 | Rendering primitives | 46 | todo |
| 31 | GPU and graphics resources | 35 | todo |
| 32 | Shader APIs | 24 | todo |
| 33 | Texture and sprite APIs | 29 | todo |
| 34 | Models and animations | 50 | todo |
| 35 | HUD and overlays | 32 | todo |
| 36 | GUI and screen system | 45 | todo |
| 37 | GUI widget library | 47 | todo |
| 38 | Input handling | 58 | todo |
| 39 | Camera APIs | 39 | todo |
| 40 | Audio and sound | 43 | todo |
| 41 | Particles and visual effects | 31 | todo |
| 42 | Networking and connections | 43 | todo |
| 43 | Packet interception and protocol APIs | 36 | todo |
| 44 | Custom networking between mods | 26 | todo |
| 45 | Commands and command registration | 30 | todo |
| 46 | Chat and messages | 34 | todo |
| 47 | Scoreboards, teams, and statistics | 39 | todo |
| 48 | Enchantments, attributes, and equipment | 28 | todo |
| 49 | Loot tables and drops | 23 | todo |
| 50 | AI and mob behavior | 34 | todo |
| 51 | Redstone and block signals | 18 | todo |
| 52 | Fluids | 21 | todo |
| 53 | Villagers, trading, breeding, and farming | 31 | todo |
| 54 | Structures and block entities | 34 | todo |
| 55 | Data formats and serialization | 25 | todo |
| 56 | Data components and custom data | 20 | todo |
| 57 | Persistence and saved data | 23 | todo |
| 58 | Resource packs and data packs | 28 | todo |
| 59 | Audio, visual, and gameplay registries | 47 | todo |
| 60 | Game rules and gameplay hooks | 30 | todo |
| 61 | World and entity queries | 35 | todo |
| 62 | Math and geometry | 31 | todo |
| 63 | Time and clocks | 26 | todo |
| 64 | Weather, time, and environmental effects | 26 | todo |
| 65 | Game settings and client options | 29 | todo |
| 66 | Window and platform APIs | 49 | todo |
| 67 | Performance monitoring and profiling | 49 | todo |
| 68 | Memory management | 21 | todo |
| 69 | Debug and developer tools | 41 | todo |
| 70 | Testing API | 37 | todo |
| 71 | Hot reload and development mode | 34 | todo |
| 72 | Scripting and embedded extensions | 28 | todo |
| 73 | Inter-mod communication | 25 | todo |
| 74 | Reflection and internal access | 32 | todo |
| 75 | Patching and transformation | 23 | todo |
| 76 | Data-driven systems | 14 | todo |
| 77 | Custom entities and projectiles | 32 | todo |
| 78 | Custom blocks, items, and content builders | 43 | todo |
| 79 | Custom menu and screen builders | 24 | todo |
| 80 | Resource and API migration | 20 | todo |
| 81 | Error handling and recovery | 18 | todo |
| 82 | Crash handling and crash reports | 20 | todo |
| 83 | Mod installation and updates | 27 | todo |
| 84 | Mod package format | 15 | todo |
| 85 | Loader configuration and global services | 31 | todo |
| 86 | Safe mode and recovery startup | 17 | todo |
| 87 | Feature flags and experimental APIs | 18 | todo |
| 88 | Plugin APIs for tools and editors | 26 | todo |
| 89 | Visual mod editor APIs | 31 | todo |
| 90 | Documentation and API discovery | 35 | todo |
| 91 | Networking diagnostics and replay | 22 | todo |
| 92 | Multiplayer and server integration | 27 | todo |
| 93 | Permissions and roles inside multiplayer | 20 | todo |
| 94 | Accessibility | 19 | todo |
| 95 | Notifications and toast messages | 18 | todo |
| 96 | Update checks and release channels | 20 | todo |
| 97 | Security, integrity, and trust | 21 | todo |
| 98 | Telemetry and opt-in diagnostics | 17 | todo |
| 99 | Save states and snapshots | 20 | todo |
| 100 | Build system and mod compilation | 31 | todo |
| 101 | API for resource generation | 20 | todo |
| 102 | Networking and cross-version translation | 25 | todo |
| 103 | Plugin bridges and foreign APIs | 17 | todo |
| 104 | Miscellaneous utility APIs | 44 | todo |
| 105 | Important API design additions | 0 | todo |
| 106 | Example API layout | 0 | todo |
| 107 | What to implement first | 0 | todo |

## Section notes

### 3 — Versioning and compatibility

`register_rule`, `register_version_mapping` and `register_compatibility_patch` are `n/a`:
the loader resolves what every mod declared before any of them loads, so a mod cannot
inject a rule into that check, and a "patch" would be a no-op on the version it claims to
fix. Version matching, mapping included, lives in `compat::check`, which both sides call.

### 6 — Event system

The bus is per-mod and driven only by the loader's own phases: `game_started` (end of
`load`), `mod_loaded`, `client_tick` + `game_tick_start`/`game_tick_end`, `render_tick`,
`settings_changed`, `input`, `mod_unloaded`, `mod_enabled`/`mod_disabled`, `game_stopping`,
`crash`. That covers what a mod running in this client can observe.

The rest of the wishlist's kinds are declared and can be subscribed to, but nothing fires
them here because the client gives no hook: `server_tick`, `world_tick`, `resource_reload`,
`language_changed`, `screen_changed`, `focus_changed`, `window_resized`, `game_paused`,
`game_resumed`, `command_registered`, `registry_changed`, and `mod_reloaded` (no hot reload,
see section 4). A mod that fires one of them itself still gets it delivered.

`input` is fired for this mod's own keybinds and buttons, which cannot be triggered
headlessly, so the ordering and cancellation are proven by unit test and the delivery path
by the settings events that share it.

### 4 — Dependencies and mod load order

`load_all`, `load`, `reload`, `unload`, `reload_all`, `unload_all` are `n/a` from inside a
mod: the loader owns loading, there is no hot reload, and a mod asking to unload itself
mid-call has no meaning here. Everything that *reads* the result is implemented.

### 7 — Threading and task scheduling

`schedule_network_thread` and `NetworkExecutor` are the honest refusal: the client owns its
network thread and offers no queue into it, so they warn and return `None` rather than
running network work on the wrong thread. Everything else runs: the tick queue (drained at
the end of every tick), the render queue (drained in the frame's render phase), worker
threads, and a `WorkerPool`.

`wait` and `join` are bounded on purpose — a main-thread task cannot finish while the main
thread waits, and a mod runs on the main thread, so these answer rather than stall the
client. Results come back through `TaskResult`, read at the start of a tick.

### 8 — Logging and diagnostics

The log pipeline keeps the loader's own log as the destination and adds what the wishlist
asks for on top: a level with per-target filters, sinks that see a record before it is
written, a 256-line history, `with_context`, structured `event`, error capture, and the
macros routed through the same path.

`diagnostics::report()` is the one block a bug report needs; `run_health_check()` compares
what the mod declared with what it has (missing permissions, unsupported capabilities,
missing dependencies, errors logged, panics caught, a task queue that is not draining);
`export_report(None)` writes it into the mod's data directory.

Heavy diagnostics reads in the example are deferred to the first tick rather than done in
`load`, see the note below on the crash that produced that decision.

## Open: a wild jump on the first frame

Five client runs crashed with a wild jump (rip ~850 MB past the executable's image, in no
mapping) about two seconds after the loader hooked the panel, with the same mod that then
ran cleanly in the CLI, which has no hooks. The trigger tracks what the mod does during
`load`: the same mod with only its logging half, or only its diagnostics half, survived;
both halves together crashed, and moving the diagnostics reads to the first tick fixed it
without losing any coverage. The panel hook is the only hook in that path and is the
suspect. Repeated for the record in `cinnaloader-notes/STATUS.md`.

Separately: a client killed with `SIGTERM` faults inside the NVIDIA driver on the way out
whether or not the loader is attached, so `tools/mcp.py` now asks it to quit first and each
run's coredump situation has to be read against how the run ended.

### 10 — Mod permissions and sandboxing

Every permission in the section is answered with a `Grant`: `Always` for the two the loader
really enforces (reading and writing the mod's own files), `OnRequest` for what it can do
within its own overlay and input, and `Never` with a reason for everything the client gives
no hook for. `request`, `grant`, `revoke`, `restrict`, `prompt_user`, `check`, `has`, `list`,
`list_for_mod`, `list_requested`, `register`, `unregister`, `is_trusted`, `set_trust_level`,
`get_audit_log`, `record_access` and `get_policy` are all there and none of them pretends:
the audit log has a line per decision, and a refusal carries its reason.

Two properties are enforced rather than documented, and the second crosses the process
boundary: a mod can only ever narrow its own permissions or trust, and the loader clamps a
manifest that claims `trusted` down to `normal`, telling the mod so. Verified in a live
client: `hello_mod asked for trust trusted, which is the user's to give: clamped to normal`
in the loader's log, and the mod itself reporting `trust=normal`.

### 11 — Filesystem and storage

`fs::*` is a real filesystem API with one rule: every path is relative to the mod's own
directory, and `..`, an absolute path or a symlink out is refused with the reason. That is
the `filesystem_read`/`filesystem_write` capability made concrete — a mod gets its own
folder, not the user's disk — and the loader sets the root as it loads each mod.

All 34 names are implemented: reading, writing, appending, copying, moving, renaming,
deleting, creating files and directories, listing, walking, metadata, size, modified time,
canonicalising, atomic writes through a part file and a rename, hashing (FNV-1a, said to be
non-cryptographic), handles for streaming reads and writes, temporary files and directories,
and a polled `watch`/`unwatch` — there is no inotify here, so a change is seen within a
tick.

`compress`/`decompress` are run-length packing in this loader's own container. It shrinks
repeated data, leaves varied data much the same size, and the round trip is exact for any
input; it is not a general compressor and is not described as one.

`storage::*` is a key/value store per name beside the mod's config, in a text format a
person can read (`key = value`, values rendered the way config values are), with namespaces,
transactions, flush, backup, restore, migrate, and a packed variant for a name ending in
`.packed`. `open_database` opens the same kind of store: this host has no SQL engine, and the
name says key/value rather than pretending otherwise.

Verified in a live client: the root reported as `/tmp/cl-mods/hello_mod`, both escapes
refused (`Outside("../outside.txt")`, `Outside("/etc/cinnaloader-probe")`), a 34-byte buffer
packed to 22 and back, and the store's whole surface — `open`, `set`, `flush`, `keys`, a
namespace, a rolled-back transaction, `migrate`, `clear`, `backup`, `restore`, `stores`, a
packed store read back, and `close`.

### 12 — Resource and asset management

A resource stack: packs, each with a name, a namespace and a priority, looked up by
`namespace:path`. Two packs offering one namespace is how a resource is overridden, and
`get_overrides` says who won and who lost. `register`/`unregister`/`get`/`find`/`list`/
`exists`/`open`/`read`/`read_text`/`read_bytes`/`metadata` are lookups over that stack;
`reload`/`reload_all`/`reload_namespace` run the reload listeners that asked for the
namespace (or for everything) and re-check the stack; `validate` finds empty packs, unusable
paths and unmet dependencies; `get_missing_resources` and `get_loading_errors` say what is
wrong; `watch` fits a resource into the `resource_reload` event so a cache can drop.

A directory pack reads a mod's own `assets/` folder through the filesystem sandbox, so it
cannot reach outside the mod's directory, and `own_pack()` is that pack ready to register.

What this host cannot do, and says so rather than pretending: feed the client's renderer.
`TextureRegistration`, `ModelRegistration`, `ShaderAccess` and `CustomRendering` stay `Never`
in section 10 for exactly this reason — the client's asset pipeline is its own.

Verified in a live client: four packs in one namespace reported in priority order
(`hello-overlay` 20, `hello-demo` 5, `hello-weaker` 1, `hello` 0), `hello:lang/en_us.txt`
reported as overridden, a resource read from the winning pack with its size and hash, both a
full reload and a namespace reload running the listener, and validation naming
`hello:textures/absent.png` as missing.

### 13 — Localization

Translations per language, asked for by key, with the lookup going current language, then
fallback, then the key itself — so a missing translation reads as something a person can
still understand. Plurals are written `one|many` (or `one|few|many`, the last form being the
general case) and English rules are what this knows; a language with more forms needs its own
function, and the module says so rather than guessing.

`reload()` reads every `lang/<code>.txt` a resource pack offers, so what a mod ships in its
`assets` and what it registers at runtime are one table, and it fires the
`language_changed` event. `on_language_changed` adds a watcher, and a change of language
tells the watchers and the event bus in that order.

Number, date, time, duration and currency formatting know three conventions, chosen from the
language code, and are documented as not being ICU. `format_date` is Howard Hinnant's
civil-from-days, which is exact for every date this will see.

Verified in a live client: `de_de` and `en_us` registered, five keys read out of the mod's own
`assets/lang/`, `Hallo, Welt!` from a placeholder, `ein Apfel` / `4 Äpfel` from one plural
key, `Bye` coming from the fallback while the current language is German, the watcher fired
by the change, and `1.234.567,5`, `2023-11-14`, `01:01:01`, `1h 2m 3s` and `12,5 USD` from
the formatters.

## Already present (ABI v5)

So a section is not re-inventing these — see `cinnaloader/README.md`:

- lifecycle `load` / `tick` / `unload`, `meta!`
- settings `feature`, `slider`, `choice`, `keybind`, `button`, drawn as client controls in
  menu tabs, with `enabled` / `number` / `choice_index` / `key` / `set_enabled` /
  `set_number` and the `toggle` / `slider` / `choice` / `keybind` / `pressed` callbacks
- per-mod persistence (`<mods_dir>/<mod>.cfg`, `config` / `set_config`)
- `Timer`, `info!` / `warn!` / `error!`, `overlay_line`, `mods_dir`, `toggle_gui`
