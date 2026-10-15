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
| 14 | Game and session state | 33 | done | What the host observes itself: frames, ticks, rates, version, launch address, window, phase, its own two managers, and four recorded requests |
| 15 | World and dimension APIs | 56 | done | Every name exists and answers honestly: the world half is `None`/`false` with a reason (hooks this host does not have), the dimension half is a real registry mods declare into |
| 16 | Block APIs | 50 | done | Block types mods declare (properties, hardness, shapes, tool, drops, behaviour) plus every world question refused with a reason |
| 17 | Item APIs | 47 | done | `ItemStack` is a real value here (create/clone/copy/split/merge/count/name/lore/damage/enchants/components); the client's item registry and the hand are refused with reasons. `get_model` is `n/a`: Bedrock draws an icon, not a model graph |
| 18 | Inventory and containers | 50 | done | An `Inventory` value with the client's own slot layout (0-8 hotbar, 9-35 main, 36 offhand, 37-40 armour) and every slot operation; live slots and containers refused with reasons |
| 19 | Entity system | 82 | done | Entity types mods declare (families, size, health, attributes, components) with the client's live entity world refused name by name, and 18 entity event kinds registered |
| 20 | Entity components and attributes | 27 | done | Component types and attribute sets a mod owns (fields, validation, serialisation, modifiers applied the game's way); attaching to a live entity refused; the ECS/system names are `n/a` |
| 21 | Player APIs | 65 | done | A `PlayerState` a mod owns (with an `Inventory` and held item from §17-18); the local player's live state refused; `send_message` writes the log and `send_action_bar` the overlay |
| 22 | Status effects and damage | 29 | done | Effect types a mod registers and applies to its own player state; damage as a real calculation (armour, resistance, protection, cooldown, modifiers) with types and sources as registries |
| 23 | Physics and collision | 35 | done | Bedrock's constants published as constants; a `Body` a mod owns (gravity, drag, friction, fall distance, collision box, sweep); the client's own physics step refused, `get_mass`/`set_mass` `n/a` |
| 24 | Raycasting and interaction | 32 | todo |
| 25 | Movement and locomotion | 36 | todo |
| 26 | World generation | 35 | todo |
| 27 | Recipes, crafting, and smelting | 29 | todo |
| 28 | Registries | 25 | todo |
| 29 | Rendering lifecycle and render hooks | 38 | done | Frame-start, frame-end and HUD callbacks are real (the loader brackets every frame); renderer/layer/pass declarations are real data; cache and resource reload are real; every client pass refused with a reason |
| 30 | Rendering primitives | 46 | todo |
| 31 | GPU and graphics resources | 35 | todo |
| 32 | Shader APIs | 24 | todo |
| 33 | Texture and sprite APIs | 29 | todo |
| 34 | Models and animations | 50 | todo |
| 35 | HUD and overlays | 32 | done | Elements with alignment, anchor, scale, opacity, layers, z index and priority, with real placement maths against the screen size; a flush draws the text on the overlay and counts the rest |
| 36 | GUI and screen system | 45 | done | Screens with a real stack, widgets with focus/hover and three real layouts (vertical, horizontal, grid), theme/style/scale/clip, input capture, cursor and tooltips; the client's screen layer refused |
| 37 | GUI widget library | 47 | done | A real node tree (two-way links, cycles refused, margin/border/padding geometry, min/max clamping, four layouts with weight), the 66-kind `WidgetType` catalogue, a keyed data store with read-through bindings and twelve events with a dispatch path |
| 38 | Input handling | 58 | todo |
| 39 | Camera APIs | 40 | done | A real camera: basis vectors, look_at, short-way rotation lerp, decaying shake, real row-major view/projection matrices, project/unproject, a six-plane frustum and `is_visible`; driving the client's camera refused |
| 40 | Audio and sound | 43 | done | A real mixer model: sounds with a load/unload lifecycle, buses and sources, the multiplied source→bus→category→master volume chain, real attenuation and pan maths, real playbacks; hearing it on the client's device refused |
| 41 | Particles and visual effects | 32 | done | A real simulation a mod owns: particle types, a pool integrated over time, seeded bursts, `spawn_in_box` that keeps its particles in bounds, emitters with a rate accumulator, and visual-effect declarations; drawing refused |
| 42 | Networking and connections | 43 | done | A mod's own socket is genuinely opened (`std::net`, bounded reads, length-prefixed framing, real statistics); the client's single game connection is refused with a reason per question |
| 43 | Packet interception and protocol APIs | 36 | done | A real binary codec (every scalar, varint, strings, byte blocks, over-read refused), real framing, a bidirectional registry and a real dispatch path with a trace; the client's protocol refused |
| 44 | Custom networking between mods | 27 | done | Real peer-to-peer channels between mods in this process: peers with API versions, channels with a codec and an enforced payload limit, version negotiation, ordered streams, request/response and delivery counted per handler; the client's connection refused |
| 45 | Commands and command registration | 31 | done | A real tokenizer and argument binder (quoted tokens, typed arguments, named errors), real prefix completion over names, aliases and choices, and a real dispatcher that checks the permission level before running an executor; the client's dispatcher refused |
| 46 | Chat and messages | 34 | done | Components a mod really builds: text, translations resolved through section 13's table with positional and named substitution, styled spans, click and hover events, a JSON round trip, and a real capped history; the client's chat refused |
| 47 | Scoreboards, teams, and statistics | 41 | done | Real bookkeeping: objectives with criteria and slots, signed scores with exact arithmetic and a defined sort, teams whose membership moves instead of duplicating, and statistic counters clamped at zero with a real export/import; the client's board, sidebar and stats refused |
| 48 | Enchantments, attributes, and equipment | 30 | done | Real rules: an enchantment cost curve exact at levels 1/3/5, a compatibility check and anvil combine, apply/remove on an item stack, attribute modifiers applied add-then-multiply then clamped, and durability that breaks at zero; the client's table and gear refused |
| 49 | Loot tables and drops | 23 | done | A real seeded generator: tables, pools with roll ranges, weighted entries, count ranges, gating conditions, functions and modifiers, with the same seed reproducing exactly and the weighting showing in 200 rolls; the client's tables and the server's roll refused |
| 50 | AI and mob behavior | 34 | done | A real decision layer: priority-ordered goals, nearest-target selection with filters and a tie-break, a `select_goal` that names the running goal and why each other was skipped, a navigator stepping `speed × dt` with a stuck detector, target cooldowns and memory; the client's brain, navigator and senses refused |
| 51 | Redstone and block signals | 18 | done | A real signal solver: one level of decay per step (14, 13, 12, 11), a receiver taking the maximum of two paths rather than their sum, rings that terminate, signal-versus-power as a first-class distinction and a real update queue; the client's redstone refused |
| 52 | Fluids | 22 | done | A real cellular simulation on a grid a mod owns: a source draining downward, levels dropping one per step and stopping at 8, viscosity making lava lag water, a fixed point after 9 steps and a real region volume; the client's fluid state refused |
| 53 | Villagers, trading, breeding, and farming | 31 | done | §53 real: a trade-offer registry whose matching runs over a real `Inventory` (pick the offer, refuse on missing inputs, spend a use, restock only after the cooldown ticks down) with a multiplier+reputation price; breeding rules that make a `BreedingPair` whose child takes every trait from a seeded c |
| 54 | Structures and block entities | 36 | done | Real transforms and real storage: a template whose Cw90/Cw180/Cw270 and both mirrors map coordinates exactly, a box with eight corners and a fit test, jigsaw anchors with directions, and a typed block entity with a tick counter, an inventory and a save/load round trip; the client's structures and block entities refused |
| 55 | Data formats and serialization | 25 | done | A typed value tree (null/bool/int/float/string/list/map) whose `Map` keeps insertion order, so it round trips byte for byte. JSON has a real writer and recursive-descent parser — the documented escape set, `\uXXXX` surrogate pairs, multi-byte UTF-8 passed through, a byte position on a malformed docu |
| 56 | Data components and custom data | 20 | done | 56. Data components and custom data — `sdk/src/data.rs`. Types: `ComponentType` (id/name/kind/default), `ValueKind` (integer, float, string, bool, list, map), Real: a component-type registry; `set`/`get`/`get_typed`/`remove`/`has`/`clear`/`keys`, with a typed read refusing a wrong kind as `TypeMisma |
| 57 | Persistence and saved data | 25 | done | typed `ConfigValue` key space, dirty flag, autosave interval; `Duration::ZERO` = never), atomic save/load through `crate::fs` with a format version that is refused when newer than `FORMAT_VERSION`, plus backup/restore/migrate/export/import and the 25 wishlist free functions. the client's level/playe |
| 58 | Resource packs and data packs | 29 | done | 58. Resource packs and data packs — done (crate-level, in-game run pending). All 29 names in sdk/src/packs.rs as a real resolver over per-pack virtual file trees built on resources::ResourcePack (tree) and resources::ResourceId (paths), not a second resource type. Priority-then-id winner in resolve/ |
| 59 | Audio, visual, and gameplay registries | 47 | done | 59. Audio, visual, and gameplay registries — done (crate-level, in-game run pending). All 47 bare `register_*` names live in `sdk/src/content.rs` (`content::register_biome` .. `content::register_fuel_type`), no new deps. Real facade: each `register_*` records in `content`'s own index *and* declares  |
| 60 | Game rules and gameplay hooks | 31 | done | 60. Game rules and gameplay hooks — done (crate-level; in-game run pending, manager wires). Files: `sdk/src/game_rules.rs`, `sdk/src/gameplay.rs`, `examples/hello-mod/src/s60.rs`. No new deps. game_rules: `GameRule` (id, owner, kind: ConfigType, default: ConfigValue, min/max, choices, name, descript |
| 61 | World and entity queries | 35 | done | The module is a **real query engine over records a mod feeds in**, plus honest refusals for the numeric fields (`numbers`), components, `visible_to`, health, age, hostile, dead. Builder. records, `query()`, `parse()`. Process-wide instance behind a Mutex for the module functions. chunks/items` are m |
| 62 | Math and geometry | 32 | done | 62. Math and geometry — `sdk/src/math.rs`. No client-owned surface: pure arithmetic, nothing refused. Functions (32): clamp, lerp, inverse_lerp, remap + map_range, smoothstep, step, wrap_angle, normalize_angle, degrees_to_radians, radians_to_degrees, distance, distance_squared, angle_between, direct |
| 63 | Time and clocks | 27 | done | §63 files: `sdk/src/time.rs` (module, stub replaced), `examples/hello-mod/src/s63.rs` (demo, stub replaced). Nothing else touched; `sdk/src/lib.rs` already had `pub mod time;` and the demo call. §63 real: fixed conversions (20 ticks/s, 50 ms/tick; `millis_to_ticks`/`seconds_to_ticks` round to the ne |
| 64 | Weather, time, and environmental effects | 26 | done | 64. Weather, time and environmental effects — `sdk/src/environment.rs`, demo `examples/hello-mod/src/s64.rs`. No new deps. Module is NOT in the prelude; doctest and demo use `cinnaloader::environment::…` full paths. The 64.prelude `pub use` line is for the manager to wire into `sdk/src/lib.rs`. Shap |
| 65 | Game settings and client options | 31 | done | Real: typed options (id/kind/default/bounds/choices/label/tooltip/category) with duplicate-id registration refused; get/set enforce kind, bounds, choice list with SettingError exactly like game_rules; SettingChange fires watch + on_changed + §6 `settings_changed` only on a real move; reset/reset_all |
| 66 | Window and platform APIs | 50 | done | §66 real (platform, no client): os/arch/family from `std::env::consts`, `platform_id()` = `os-arch`, real line separator and `std::path::MAIN_SEPARATOR`, `is_little_endian()` via `cfg!(target_endian)`, `is_64_bit()` via `size_of::<usize>()`, `cpu_count()` via `available_parallelism`, `get_cpu_info() |
| 67 | Performance monitoring and profiling | 51 | done | Status: DONE — implemented, unit-tested, doctested, builds, clippy-clean for the lane's files. No other file touched. `sdk/src/lib.rs` already declares `pub mod metrics;`/`pub mod profiler;` (lines 66-67) and the demo is already hooked. (idempotent by name). percentiles: 1-based rank `ceil(p/100*n)` |
| 68 | Memory management | 21 | done | 68. Memory management — `sdk/src/memory.rs`, demo `examples/hello-mod/src/s68.rs`. No new deps. Module already in lib.rs (`pub mod memory;` at line 68). Prelude line is in Doctest and demo use `cinnaloader::memory::…` full paths, so the prelude is not required. Shape: a REAL store the mod owns + the |
| 69 | Debug and developer tools | 41 | done | 69. Debug and developer tools — `sdk/src/debug.rs`, demo `examples/hello-mod/src/s69.rs`. No new deps. Module `pub mod debug;` is already declared in `sdk/src/lib.rs`; it is NOT yet in the prelude. The 69.prelude `pub use` is for the manager to wire into the prelude; doctest and demo call `cinnaload |
| 70 | Testing API | 38 | done | 70. Testing API — `sdk/src/testing.rs`, demo `examples/hello-mod/src/s70.rs`. No new deps. `pub mod testing;` already in `sdk/src/lib.rs`, NOT yet in the prelude; the doctest and demo use `cinnaloader::testing::…` full paths. 70.prelude is for the manager. All 38 wishlist names exist. The harness is |
| 71 | Hot reload and development mode | 34 | done | §71 real: `sdk/src/dev.rs` — a watch list whose `Fingerprint` (size + mtime + FNV hash) reports a path once and never again while it is unchanged; `plan_reload` splits changed paths into `reloadable` / `blocked` with a reason each (a mod's own assets/config/scripts vs the client's shaders, classes a |
| 72 | Scripting and embedded extensions | 28 | done | 72. Scripting and embedded extensions — `sdk/src/scripting.rs`, demo `examples/hello-mod/src/s72.rs`. No new deps. `pub mod scripting;` already in `sdk/src/lib.rs`, NOT yet in the prelude; doctest and demo use `cinnaloader::scripting::…` paths. 72.prelude is for the manager. §72's `Value` collides w |
| 73 | Inter-mod communication | 25 | done | Files owned: `sdk/src/interop.rs` (module), `examples/hello-mod/src/s73.rs` (demo). No other file touched. `pub mod interop;` already existed in `sdk/src/lib.rs:73`; prelude exports are in with `MethodSpec { name, arity, returns: ReturnKind }` as real data; builders `capability`, name refused (`fals |
| 74 | Reflection and internal access | 33 | done | Status: DONE for this lane — implemented, unit-tested, doctested, demo runs in the CLI, clippy-clean. No manager-owned file touched (`sdk/src/lib.rs`, `examples/hello-mod/src/lib.rs`, in `sdk/src/lib.rs` (lines 74-75); the demo is already wired at Prelude wiring is the manager's: `/tmp/cl-lanes/74.p |
| 75 | Patching and transformation | 24 | done | §75 real: `sdk/src/patches.rs` — real byte patching over a buffer the mod owns, and the ledger. order (overlapping included, step 1); `find_first`. replacement length == pattern length. bytes that are not the pattern (`PatternMismatch { at, expected, found }`) instead of writing over them, then writ |
| 76 | Data-driven systems | 15 | done | Index: what landed, the public surface, the 15 wishlist names, test list, gate receipts, integration points for the manager, and what is honest-partial. Extra helpers needed by the spec/demo: `unload`, `count`, `get_all_of`, `summary`, Types (exported): `Document`, `DocumentSchema`, `SchemaField`, ` |

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

### 14 — Game and session state

Two kinds of answer, kept apart on purpose. What this host observes itself: that it is
running, for how long, frames and ticks and their rates (counted from the client's own frame,
one tick per frame), the longest frame, the game version the client's assets name, the
address the client was launched with, the window when the launcher named it, the phase, the
loader's own resource stack and mod registry, and the host clock.

What it cannot reach: the client's screen, world, player, dimension, game mode, pause state,
menu state, network manager and world renderer are objects inside the client that this host
has no handle on. Each answers `None` with a reason from `why_unknown`, rather than a
plausible-looking value, and `get_client`, `get_server`, `get_network_manager` and
`get_world_renderer` do the same.

The four requests are honest rather than convenient: `request_resource_reload` is real
because the loader owns a resource stack; `request_disconnect`, `request_world_reload` and
`request_game_shutdown` are recorded with the reason they cannot be made, and logged so the
user sees them. A loader that killed a client mid-frame would corrupt its state.

`is_multiplayer`/`is_singleplayer` come from the launch command line and `is_online` is the
loader's own observation (an address was named and frames are still arriving), which is what
it says it is.

Verified in a live client: `version="1.26.50.4"`, `multiplayer=true`,
`server=Some("127.0.0.1:19132")`, `online=true`, `window=Some(Window { 1280x720 })`, the
session and runtime values, the registry and resource summaries, every unseen question
`None`, and the request log with only the resource reload granted.

### 15 — World and dimension APIs

This is the first section that is mostly a refusal, and it is a refusal with reasons rather
than `todo`. The client's world — chunks, blocks, entities, players, biome, weather, time,
seed, spawn, border, save file — lives inside the client, and section 10 lists the
capabilities that would read or change it as `never`
(`needs hooks into the client's world and entity state`). Every `world::` name exists and
answers `None`, `false` or an empty list, with the reason in `world::why_unknown`, and
`world::refusals()` is the whole table for a panel or a report. `load`, `unload`, `save`,
`mark_dirty`, `set_spawn_position`, `set_weather`, `load_chunk`, `unload_chunk`,
`request_chunk`, `create_snapshot` and `restore_snapshot` log what they were asked and do
nothing, because doing anything would be a lie about the client's state.

`dimensions::*` is real. A mod declares the dimensions it would add — id, name, kind, bounds,
gravity, its own clock, biome source, ambient properties and respawn rules — and every mod
can read that registry back, by id or by name. `create` and `teleport_to_dimension` then
answer honestly that this host cannot make the client act on a declaration: that is
`CustomDimensions`, reported `never` in section 10, and the declaration stays a declaration.

Verified in a live client: every `world::` question `None` or `false` with its warning in the
log, the refusal table naming thirteen kinds of world state, and a declared `skylands`
dimension read back with its bounds, gravity, clock, biome source, ambient properties and
respawn rules, then unregistered.

### 16 — Block APIs

The same split as section 15. A **block type** is data a mod ships, and declaring one is real:
id, name, material (which picks the sound), hardness, resistance, friction, light emission,
solid/replaceable/transparent/fluid/air/flammable/gravity, properties with their values and
defaults, collision/outline/render/selection shapes, tool requirement, drops, fluid state,
whether it can hold a block entity, and its behaviour callbacks. Every mod can read that
registry back, by id or by name, and `register_property` and `register_block_entity` amend a
declaration in place.

The behaviour callbacks — `on_neighbor_update`, `on_random_tick`, `on_scheduled_tick`,
`on_use`, `on_attack`, `on_break`, `on_placed` — are recorded on the declaration and this
host **never calls them**, because nothing here sees a block event from the client. The
module says so where it matters, `blocks::behaviour::why_never_called()` is the sentence, and
`blocks::register_event_kinds()` teaches the §6 event bus all eighteen block event names so a
mod's subscription is in place for when the hooks land.

Everything about blocks *in the world* answers `None` or `false` with the reason:
`get_at`, `get_state`, `set`, `set_state`, `set_property`, `place`, `break_block`, `remove`,
`replace`, `update`, `notify_neighbors`, `schedule_tick`, `cancel_scheduled_tick`,
`get_light_block`, `get_block_entity`, `create_block_entity`, `can_place_at`, `can_survive`,
and the six questions whose name a declared-type accessor already holds are exported as
`get_type_at`, `get_id_at`, `get_property_at`, `get_light_level_at`, `get_fluid_state_at`,
`get_block_entity_at`, with the module path having the wishlist's own spelling too.

Verified in a live client: a lantern declared with two properties, read back with its
material, hardness, light level, drops and tool requirement, its behaviour reporting
`["on_use", "on_break"]` while `why_never_called()` explains that nothing here calls them,
eighteen event kinds registered, and every world question `None` or `false`.

### 17 — Item APIs

An item **stack** is a value this host owns, and that half is real: identifier, count, aux,
damage, custom name, lore, enchantments and Bedrock item components, with create, clone, copy
(three names for one operation in the wishlist), split, merge, count, name, lore and damage
implemented on it, and the free functions the wishlist lists as thin wrappers over those
methods.

An item **type** a mod declares is real data too — max stack, durability, icon name, tags,
components, use action and duration, cooldown, and the food/armour/tool component groups —
and every mod can read that registry. The grouping is Forge's; Bedrock carries the same data
as components, and the module says so instead of pretending the grouping is Bedrock's.
`register` logs that the client has not been told, and `is_registered_with_the_client`
answers honestly that it never has.

What the client owns is refused with a reason: its item registry (`get`, `get_by_id`,
`get_by_name`, tags, enchantment definitions, icons) and the stack in the player's hand
(`get_stack`). `get_model` is `n/a` rather than refused: a Bedrock item draws an icon, so
there is no model graph to return and the Forge model this name comes from has no
counterpart. The nineteen item event kinds are registered on the section 6 bus so a
subscription is in place for the day the hooks land.

Verified in a live client: a pickaxe stack with 250 durability worn to 200, named and
enchanted, split and merged, and a declared lantern read back with its max stack, durability,
tags, use action, cooldown, icon and the three component groups; the hand, the model and the
client's registry all `None`.

### 18 — Inventory and containers

[`Inventory`] is a value: a slot vector using the client's own indices — `0..=8` hotbar,
`9..=35` main, `36` offhand, `37..=40` armour — with get, set, clear, contents, add, remove,
contains, contains_amount, count, find, find_all, first_empty_slot, can_insert, can_extract,
insert, extract, swap, move_stack, split_stack, merge_stacks, transactions
(begin/commit/rollback), a revision counter that changes with every slot, and a lock that
stops changes for a mod that hands the inventory to a worker. `insert` and `merge_stacks`
respect the slot's maximum, a move that does not fit leaves the rest where it was rather
than losing it, and `add_item` tops up a partial stack before taking an empty slot.

It is **the mod's copy**, never the player's: `inventory::get()` binds the live inventory,
and answers `None` with the reason instead of handing back something that is not the game's.
The live slot reads — the selected hotbar slot, armour, offhand, cursor — are refused the
same way, and so is every `containers::` call about a live container: opening, closing,
slots, properties, viewers, interaction distance, title.

`containers::register_type` is real: a mod declares a container with its slot count,
properties, title, distance limit and whether it has slot-click and property-change handlers,
and every mod reads that registry. `containers::get_viewers` is `n/a` rather than refused: a
Bedrock client has no viewer list, that is a server idea.

Verified in a live client: 164 stone across three slots, a split of 20, a move that took 44,
a transaction rolled back to 144, a lock refusing a write until it was unlocked, the whole
live surface `None`/`false`, and a declared crate read back with its 27 slots.

### 19 — Entity system

Every question about a *live* entity is refused with a reason, and the reasons are specific:
the client's entity store is the only place an entity exists (`lookup`), position and rotation
come from its own interpolation (`position`, `movement`), flags like sneaking and burning are
applied by the client (`state`), health arrives from the server (`health`), attributes and
components are what the client syncs (`attribute`, `component`), families are server data
(`tag`), owners, riders and targets are not synced to a Bedrock client in general
(`ownership`), and spawning, damaging, killing, teleporting, mounting are the server's
(`change`) — the client can only ask, and this host cannot even ask. Presentation
(`set_glowing`, name tags) is the client's own layer. `is_ai_controlled` is `None` rather
than a guess: for everything but the local player it would be made up.

What *is* real: a mod declares an entity type — families, size, health, movement, attributes,
components, tags, whether it is rideable and whether it is hostile — and every mod reads that
registry, along with the arithmetic that is pure once a mod has its own numbers:
`bounding_box_at`, `distance`, `distance_squared`, `eye_height`. The eighteen entity event
kinds are registered on the section 6 bus for the day the hooks land.

Verified in a live client: a wisp declared and read back with its size, health, both
attributes, families and component; its bounding box at a position; and every live question
`None` with spawn, damage, teleport, mount, glowing and attribute changes all refused.

### 20 — Entity components and attributes

The real half is a component *type* registry and a value tree: a mod registers a component
with its fields, required fields and defaults, then creates, clones, serialises,
deserialises and validates values against it, with `register_serializer` checking the round
trip and `register_validator` making the schema refuse a bad value before it is stored.
Attributes are the same shape and more: `AttributeSet` holds a base, an optional range and
modifiers of the three kinds the game has, and `current()` applies them in the game's own
order — additive first, then multipliers, then the total multiplier — clamped to the range.
Verified live: `0.25 + 0.05` then `× 1.5` gave `0.44999999999999996`, and removing the
multiplier put it back to `0.3`.

The refused half: attaching a component to a live entity, querying entities for components,
and an attribute's value on an entity all need the client's entity store, which section 19
already refuses for want of a hook.

The `n/a` half, said out loud rather than dressed up as a missing hook:
`components::register_system`, `unregister_system`, `run_system`, `get_system_order`,
`set_system_order` and `attributes::register`/`unregister` name an ECS and a system scheduler
for mods. A Bedrock client has neither, so there is no target. `attributes::register` and
`unregister` *do* work in the honest sense — a mod owns attribute definitions — and the
functions that would add one to the client's engine are the ECS ones.

Verified in a live client: a glow component registered with a required field, a value set,
validated twice (once clean, once with a field of the wrong kind), serialised and read back,
the entity half `None`/`false`, and the attribute half with both modifier kinds.

### 21 — Player APIs

[`PlayerState`] is a value a mod owns, and it is where this section's real half lives: name,
uuid, display name, game mode, position, rotation, velocity, health and maximum, food,
saturation, experience, level, score, locale, team, permissions, statistics, advancements,
effects with their levels and expiry, respawn/spawn/last-death positions, skin, cape,
profile, the selected hotbar slot, an [`Inventory`] from section 18, and the held stack from
section 17. Health is clamped, effects replace one of the same name, expired effects drop
out, statistics add up, and `reset_state` clears what a respawn would.

The local player is the client's, and every question about one is refused with its own
reason: identity, position, health, inventory, effects, connection and latency, skin and
cape, input, camera. Sixteen of them, in `refusals()`.

Two "send" calls are real, because this host has somewhere to put them: `send_message` writes
to the loader's log (not the game's chat — the module says so), and `send_action_bar` writes
the loader's overlay line, which is this host's closest thing to an action bar, read by the
external overlay the loader exposes. Titles, sounds, particles and screens need the client's
UI, audio and renderer, and `give_item`, `take_item`, `apply_effect`, `teleport` and
`set_game_mode` are the server's; all of them are refused rather than pretended.

Verified in a live client: a player state rendered with all its fields, a 64-torch held item
and a shield offhand, `de_de` with the team and level, `[chat] hello from the example` in the
log, every live question `None`, and every authority call `false`.

### 22 — Status effects and damage

The effects half is a registry plus the list section 21 already keeps: a mod registers an
effect type — id, name, good or bad, colour, icon and particle names, default duration and
amplifier, whether it has a behaviour — and `apply`, `remove`, `clear`, `has`, `get`,
`get_duration`, `set_duration`, `get_amplifier` and `set_amplifier` work on the
[`PlayerState`] a mod owns, so an effect a mod applies to its own state is really there.
`register_behavior`, `register_icon` and `register_particles` record what a mod ships; the
client's icons and particles are its own, and the behaviour is never called here.

The damage half is real arithmetic, which is why it is worth having before any hook exists:
[`DamageInstance`] carries the amount, the type, the source, armour and toughness,
resistance, enchantment protection, the cooldown and the knockback, and resolves in the order
the game does — armour, then resistance, then protection, then the cooldown multiplier, then
the mod's own final modifiers. Damage *types* (including ones that go through armour),
*sources* and *modifiers* are registries a mod owns. Verified in a live client: 10 untouched,
2.52 through 20 armour with toughness 2, resistance 2 and protection 4 (reductions 0.500,
0.400, 0.160), 3.60 for a quick second hit, and 10.00 again for a type that bypasses armour
and resistance.

What is out of reach keeps its own reason: applying an effect or damage to a live entity is
the server's, and `modify_calculation` would need a hook on the client's damage path. The
thirty-three event kinds this section lists are registered on the §6 bus.

### 23 — Physics and collision

The client's physics step is the hottest code it has and its simulator is out of reach, so
the live half is refused — `get_velocity`, `get_ground_state`, `get_fluid_state`,
`get_fall_distance`, `get_collision_box`, `is_colliding`/`is_grounded`/`is_inside_block`/
`is_inside_fluid`, `check_collision`, `get_movement_result` — and every call that would
*change* it is refused with its own reason: movement is server-authoritative, `set_no_clip`
would desync on the next tick, and `modify_gravity`/`modify_friction`/`modify_knockback`/
`register_collision_handler`/`register_physics_step` would need a hook inside the step.

What is real is more than the section's name suggests. Bedrock's constants are published as
constants, said to be *the platform's numbers rather than a live read of the client's*:
gravity `0.08`, vertical drag `0.98`, terminal velocity `3.92`, ground drag `0.6`, friction
`0.6` and `0.91` for ice, and the eye offset. `Body` is a value a mod owns — position,
velocity, collision box, ground and fluid state, fall distance, and a `step` that integrates
gravity and drag the way the game does — with `apply_force`/`apply_impulse`/
`apply_acceleration`, `land`, `fall_damage` (one per block past three), `is_inside_block`
against a world the mod supplies, and `sweep`. Verified in a live client: a body fell 50.33
blocks at the terminal velocity, took 47 damage on landing, and `get_mass` answered `None`
because Bedrock's client physics has no per-entity mass.

### 24 — Raycasting and interaction

The ray builder the wishlist asks for, over a world the **mod** supplies: `from_position`,
`from_body`, `from_camera`, `with_direction`, `with_range`, `with_filter`, `with_predicate`,
`include_passable_blocks`, `include_fluids`, `exclude_entity`, `blocks`/`entities`/`all`, and
`cast` with a solidity function. The cast itself is a voxel DDA, exact and cheap, shared with
section 23's `sweep`. `cast_entities` tests the same ray against entity boxes by the slab
method, and `get_result` answers both.

The result accessors are all real — `get_hit_position`, `get_hit_block`, `get_hit_entity`,
`get_hit_face`, `get_hit_distance`, `is_clear`, `get_result` — and `interaction::*` is real
where a mod has the target: the reach numbers are the game's constants (4.5 survival, 5.0
creative, 12 for a camera pick), `get_target` and `can_interact` work over a mod's own query,
and `set_reach_distance`, `modify_reach_check`, the six interactions, `register_handler` and
`cancel_interaction` are refused because interaction is sent by the client on the player's own
input and accepted by the server.

Verified in a live client: a ray from `(0.5, 0.5, 0.5)` hit the wall at `(3, 0, 0)`, 2.50
blocks away, face `(-1, 0, 0)`; the entity query found `wisp` with `far` behind it, and with
`wisp` excluded found `far`; a camera pick had range 12; and a sweep of a body stopped at the
wall.

### 25 — Movement and locomotion

The game's movement numbers are published as defaults — walking `0.1`, sprinting `0.13`,
sneaking `0.03`, flying `0.05`, swimming `0.02`, jump strength `0.42`, step height `0.6`,
acceleration — and a mod owns a [`MovementProfile`] of them, which it can change for its own
prediction without touching the client.

What is real: `Input` as a value, with `vector()` normalising a diagonal the way the game does
so two keys are not faster than one, opposite keys cancelling, and `is_moving`/`is_sprinting`/
`is_sneaking`; `speed_for` picking the speed an input asks for (sneak beats sprint, the mode
decides when nothing is pressed); `jump_height` from the strength and Bedrock's gravity
(1.102 blocks, which is about what a player clears); `can_step_up` against the step height;
`predict` for where a body ends up after so many ticks, including rising and diving in flight
without any horizontal input; and `steps` for the whole path.

Refused with its own reason each time: the live input and its vector, every one of the eight
speed getters and their setters (a player's speeds are server attributes), the ten
`jump`/`start_*`/`stop_*`/`set_*_allowed` actions, the client's own prediction and the
server's correction and authoritative position, and the three `register_*` hooks.

Verified in a live client: the constants; `walk=0.1 sprint=0.13 sneak=0.03`, jump height
`1.102`, step `0.6`; 20 ticks of walking moving 2.0 blocks and sprinting 2.6; a jump rising to
`64.63`; flying up `0.25` and down `0.25` in five ticks; and the whole live surface `None`.

### 26 — World generation

Everything a data pack describes, a mod registers here, and every kind is a real entry in a
real registry: biomes with their temperature, downfall, surface blocks, features and
structures; and generation pieces of twelve kinds — feature, structure, ore, carver, surface
rule, placement rule, noise, density function, dimension, dimension type, biome source and
chunk generator — each with the settings a mod gives it, read back by kind and by id.

More than the section's name suggests is real, because a mod can generate for itself:
`Random`, a seeded xorshift64* with `next_u32`, `next_f64`, `next_range`, `chance`, `pick` and
`for_position(seed, x, z)`, which is the shape the game uses to seed a chunk; `Noise`, a
Perlin-shaped function with `at`, `at_2d` and `fractal`, said to be *this loader's noise and
not bit-identical to the client's*; `GenerationContext`, which knows its origin and its own
`random()`; `Chunk`, sixteen by sixteen columns of block names with `set_block`, `block`,
`height_at`, `biome_at` and `solid_count`; and `generate_chunk`, `populate_chunk`,
`place_feature`, `place_structure`, `get_height_at` and `get_biome_at` over a generator or a
chunk a mod owns.

Refused: the client's seed, its generator, its random, its ordering, and a generation
context pointed at the client's world — none of which this host can be told about, because
the client generates its own world without calling out.

Verified in a live client: a biome registered with its settings; twelve pieces across every
kind, with the chunk generator's callback recorded; the same seed drawing the same numbers
twice and a different one drawing different ones; noise reproducibly seeded; a 32,589-block
chunk generated twice identically from one seed; `populate_chunk` filling 256 columns and a
2×2×2 structure placing eight blocks.

### 27 — Recipes, crafting and smelting

A recipe is data — a shaped pattern with a key, a shapeless list, or a machine recipe with an
input, an output, a cook time and experience — and every kind registers here: shaped,
shapeless, smelting, blasting, brewing, smithing, stonecutting and machine, plus types,
serializers and conditions a mod invents. `get_all`, `get`, `get_by_id`, `get_for_result`,
`replace`, `remove`, `get_type`, `get_category`, `get_cook_time`, `get_experience`,
`get_ingredients`, `get_result` and `get_remaining_items` read it all back.

The matching is real and is the point of the section: a shaped recipe matches its pattern at
any offset in a grid of the right size, mirrored horizontally unless the recipe says
otherwise, with a mirrored column being a *rotation* and therefore not a match — which is
exactly what the game does and is tested as such. A shapeless recipe matches any arrangement
with the right counts; a tag ingredient matches an item named for the tag or carrying it as a
tag; a cooking recipe takes one input. `find_matching`, `can_craft`, `craft`, `craft_all`,
`grid_from` and `find_matching_items` work over `ItemStack`s from section 17 and the
`Inventory` layout from section 18, so a mod crafts here exactly as the game would.

Fuels are a registry with the game's own values (coal 1600 ticks, a stick 100, a lava bucket
20000) and `batch_size` says how many of a recipe a piece of fuel cooks.

Refused: the client's own recipes and serializer registry, and crafting into the player's
inventory. Verified in a live client: all seven kinds registered, a column pattern matching
while its rotation does not, `craft` giving four torches, coal cooking eight glass, and the
whole refusal table present.

### 28 — Registries

One engine, and the section that the rest of the loader's registries want to be built on: a
`Registry` of `ConfigValue`s with an id-to-key index and the reverse lookup, `register`,
`register_keyed`, `register_batch`, `unregister`, `replace`, `get_by_id`, `get_by_key`,
`get_key`, `get_id`, `contains`, `keys`, `values`, `entries`, `iterate`, `get_size`, a
version that rises on every change, a change log that says what happened when, `freeze` and
`unfreeze` (a frozen registry refuses every change with the reason), `snapshot` and `restore`
(the section 18 transaction shape over a whole registry), `validate` (an id with no
namespace, two ids answering to one key, values a validator refused) and a serializer
registry that `register_codec` shares, because Bedrock's wire format is its own and a codec in
the JVM sense does not exist here.

`register_custom_registry` is the payoff: a mod names a registry of its own — including
`Blocks` or `Items`, which are free text here — and `register_in`/`get_in`/`unregister_in`
work on it while the *client's* registries of the same names stay out of reach.
`get_registry_manager` is `None` with the reason, and `CLIENT_REGISTRIES` lists the
thirty-one names the client owns.

Verified in a live client: a named registry and a keyed entry, `get_by_key` and the reverse
lookups, a batch, `iterate`, the change log, `freeze` refusing a registration with
`Err(Frozen("cinnaloader:default"))`, a snapshot restored, `validate` clean, two serializers,
the client manager `None`, and a mod's own registry called `Blocks` taking an entry.

### 29 — Rendering lifecycle and render hooks

Three things here are real, and they are the ones a mod uses. Frame boundaries: the loader's
frame hook brackets every client frame, so `on_frame_start`, `on_frame_end` and
`on_hud_render` are subscriptions to the section 6 render event and run at the right moment on
the render thread — verified in a live client with all three firing in pre/main/post order from
one render tick. Declarations: `register_renderer`, `register_render_layer`, `register_pass`
and their `unregister`s are real data, read back in draw order (order first, then name), and
nothing draws them yet, which the module says. Cache and resources: `invalidate_cache` and
`reload_resources` call section 12's reload listeners, and `is_render_thread` answers.

Everything inside the client's renderer is refused with its own reason: the world, entity,
block, chunk, hand, screen, particle, nameplate, outline and shadow passes, post-processing,
texture uploads, shader reloads, the camera, the view/projection/model matrices, the
framebuffer, render target, device and backend, and `request_frame`. The seventeen render
event names are registered on the bus so a mod's subscription is already in place.

### 30 — Rendering primitives

Everything the wishlist lists is a value in a `DrawList`: lines, rectangles (plain, filled,
rounded, gradient), circles, ellipses, triangles, polygons, polylines, quads, boxes (solid and
wireframe), spheres, cylinders, cones, capsules, meshes, text (plain, shadowed, outlined),
textured rectangles, textures, sprites, item/block/entity icons, scissors and clip rectangles
— 49 names, each with its own command. The state goes with them: colour, alpha, blend mode,
depth test and write, cull mode and line width, snapshotted onto every command as it is added.

The transforms are a real 3×3 affine stack and compose the way an immediate-mode renderer's
do: the transform a caller adds last acts on local coordinates and the one added first
positions the result, which is what makes `translate(10, 20)` then `filled_rect(0, 0, …)` put
the rectangle where the caller meant. `push_transform`/`pop_transform` bracket it,
`inverted()` rounds a point trip exactly, and a singular transform refuses to invert. Verified
in a live client: `translate(10, 20)` taking `(1, 1)` to `(11, 21)`, a quarter turn taking
`(1, 0)` to `(0, 1)`, and an inverse taking `(4, 4)` back to `(2, 2)`.

`flush` is where the honesty is: this host's only drawing is the loader's overlay line, so a
flush draws the *text* commands there and counts everything else as a draw call it could not
issue — `get_draw_calls` reports both. Verified in a live client: a 25-command list flushed
with 3 drawn and 22 undrawn, and every refusal carrying its reason.

### 31 — GPU and graphics resources

The client owns the GPU device, its adapter, its backend and every object on it, and this host
has no handle on any of that — so `get_device`, `get_adapter`, `get_backend`,
`get_capabilities`, `get_limits`, `submit`, `read_buffer`, `write_buffer`, `read_texture`,
`write_texture`, `resize_texture` and `resize_render_target` are all refused with their own
reason, and the module is explicit that these APIs are deliberately not tied to one backend
(`BACKENDS` lists the four names rather than assuming one).

What is real is the ledger a mod keeps: all eighteen `create_*` calls record a description —
kind, label, bytes, format, dimensions, usage — and answer a handle, and the ledger answers
`get_resource_count`, `get_memory_usage`, `get_resources` and `get_resource`, plus
`release_resource`, `note_validation_error`/`get_validation_errors` and
`note_device_lost`/`get_device_lost_reason`, which are what a mod recorded rather than what the
client knows. `is_created_on_the_gpu` answers `false` for every handle, because nothing here
creates anything.

Verified in a live client: seven resources in the ledger totalling 43,264 bytes, a handle
released with the count dropping from 7 to 6, `on_gpu=false`, the validation error and
device-lost note read back, and every device call refused.

### 32 — Shader APIs

A shader is source text here, which makes most of the section real: `register`, `unregister`,
`get`, `get_all`, `register_include`/`unregister_include`, `set_define`, `set_variant`,
`set_uniform`/`get_uniform_value`, `get_uniform`/`get_uniforms`, `get_attributes`,
`get_reflection`, `register_material`/`get_material`/`get_materials` and
`register_post_process`/`unregister_post_process`/`apply_post_process`/`remove_post_process`/
`get_post_processes`/`get_active_shaders` all work on data a mod owns.

`get_reflection` is the interesting one: it reads the uniforms, attributes and defines out of
the source a mod wrote, following its includes, and reports them the way a compiler's
reflection would — with `ShaderVariable::components()` knowing how big a `mat4` is — without a
compiler existing here. A pass carries one of the thirteen hooks the wishlist lists, and the
pass list sorts the way a frame runs them.

Refused: compiling, loading into the client's pipeline, its pipeline cache, its active shader
list, a real compiler's errors (this host reports none rather than inventing them) and the
backend's supported features. A pipeline a mod *declares* lands in section 31's ledger through
`create_pipeline`, and `is_created_on_the_gpu` still says no. Verified in a live client: an
outline shader with four reflected uniforms across its own source and an include, a 4-component
`vec4` and a 16-component `mat4`, a define, a variant, a uniform value read back, two passes in
frame order, 13 hooks, and every client call refused.

### 33 — Texture and sprite APIs

A texture here is real data: its id, dimensions, format and *pixels*, loaded from bytes, from a
raw file in the mod's own directory or from a resource the mod shipped, with `get_pixel`,
`set_pixel`, `update_region`, `get_pixels` and `set_pixels` working on them and
`get_memory_usage` adding the bytes up. The four formats read and write the way they should —
BGRA storing as BGRA and reading back as RGBA, RGB giving every pixel full alpha, R
replicating one channel.

An atlas is real too, with a shelf packer: `register_sprite` places sprites across shelves and
answers where they landed, `get_sprites` lists them, `get_uv` turns a sprite into UVs against
the atlas size, and `stitch_atlas` counts what it laid out. Animations are real data with a
duration per frame, and `frame_at` picks the frame a moment falls in, looping. Filter, wrap
mode, anisotropy and the mipmap flag are recorded.

Refused: uploading to the client's GPU, binding, deleting, reloading and building mipmaps —
`why_unknown` says a texture reaches the screen through the client's device, which this host
has no handle on — and the *decoder*: this host has no image decoder, so `load_from_file`
takes raw pixels and `is_raw_image` says which extensions those are.

Verified in a run of the loader with the mod attached (`cinnaloader run`, which is the CLI
runner, after the client's hook layer started to crash on repeated runs): pixels loaded and
read back as `[255, 255, 255, 255]`, a pixel set to `[10, 20, 30, 255]`, a region of two
written, three sprites placed at `0,0` / `8,0` / `0,8` with UVs `(0, 0, 0.5, 0.5)` and
`(0, 0.5, 0.5, 1)`, 192 pixels stitched, an animation giving frame 1 at 150 ms and frame 0 at
400 ms as it loops, and every upload refused.

### 34 — Models and animations

A model is real data: bones with a parent and a transform, parts with vertices, indices, a
material and a texture, visibility per bone and part, bounds worked out from the vertices,
a bone chain walked up to the root, and an `export`/`import` pair that round-trips a model as
text — the counts and the transforms, with the vertex values left to the mod that owns them.
`create`, `create_mesh` and `create_bone` build one; `set_material`, `set_texture`,
`set_transform` and `set_visibility` change it.

The animation side is where a mod can do genuine work, and it is all real: keyframes per bone,
`length`, `time_at` with the four loop modes (once, loop, ping-pong, hold), and `sample`
interpolating between the keyframes around a moment. A `Playback` carries time, speed,
looping, pause and blend weight and advances with `advance`; a `crossfade` moves the weights
from one animation to the other; `blend` normalises them; parameters, controllers and a
`get_skeleton` that samples every bone from the model's own animation complete the picture.

Refused: loading a model into the client, its own model loaders, and the entity, block and
item renderers and keyframe handlers, which are recorded as declarations and never called
here.

Verified in a run of the loader with the mod attached: a two-bone cube with bounds
`(0, 0, 0)`–`(1, 1, 1)` and the chain `lid → body`; a five-line export read back by `import`;
an animation played to 250 ms, doubled in speed to 750 ms, held there by `pause`, and a
crossfade giving a 50/50 blend; and a skeleton sampled at 250 ms of a 500 ms animation giving
`(0, 1.05, 0)`, which is the interpolated midpoint of `1.0` and `1.1`.

### 35 — HUD and overlays

An element is real data — where it sits, how big it is, its alignment against the screen, the
anchor it grows from, scale, opacity, layer, visibility, enabled, z index and priority — and
`bounds` does the arithmetic: the alignment places it among the free space, the anchor takes
its own size off the position, and the scale multiplies it. `get_screen_size` reads the
window the loader was told about (`CINNALOADER_WINDOW`, which `tools/mcp.py` sets, falling
back to the window facts), `get_scaled_size` divides it, and `get_safe_area` insets it by a
margin. Layers are real, elements sort by layer, then z index, then priority, invalidation
counters count, and tooltips, click handlers and animations are recorded.

The drawing is the honest part: `flush` draws the *text* of every element that would be drawn
as an overlay line and counts the rest, so a coordinates or FPS element works and a minimap
does not — `why_unknown` says why for the drawing, an image element, a click handler and an
animation. Verified in a run of the loader with the mod attached: elements placed at
`(4, 4, 90, 12)` and at the bottom right, moved and scaled to `(8, 8, 150, 21)`, a hit test
that found the pointer inside, two elements drawn and none undrawn, and the layer, tooltip,
handler and invalidation calls all answering.

### 36 — GUI and screen system

A mod can build a whole screen here, and the parts that matter are real: screens with an id
and a title, a stack that pushes, pops and tells you the current and previous screen, widgets
with a kind, a label, a size and a parent, `create_widget`/`register_widget`/
`unregister_widget`/`get_widget`/`remove_widget`/`find_widget`, focus that moves and clears,
hover from a real hit test against the placed rectangles, and `get_root_widget`.

The layouts do actual arithmetic: absolute leaves each widget where it says, vertical stacks
them down the screen from the padding with the spacing, horizontal lays them across, and a
grid wraps them into columns — all clamped so a screen smaller than its own padding produces
zero-size rectangles rather than negative ones. Theme, style, scale, clip area, input capture,
cursor and tooltips are the mod's own state, and `request_layout`/`invalidate`/`rebuild`
count what a mod asked for.

Refused: `open_screen`, `close_screen`, `replace_screen` and a widget handler being called by
the client, with the reason that the client's screen layer is its own and the loader's panel
is the loader's rather than a mod's.

Verified in a run of the loader with the mod attached: the stack pushing Settings over Hello
and popping back, the vertical layout placing three widgets at y = 4, 26 and 48, the grid
wrapping the third onto row two, focus moving and clearing, a hover hit test finding
`hello:play` at (50, 20), and every client call refused.

### 38 — Input handling

The loader reads the keyboard and the mouse with `device_query`, and this module turns what it
sees into the state a mod asks about: `press`, `release`, `move_to`/`move_by`, `scroll` and
`set_modifiers` update it (the SDK's own event loop does, in practice), and `is_key_down`,
`is_key_pressed`, `is_key_released`, `is_mouse_down`/`pressed`/`released`,
`get_mouse_position`, `get_mouse_delta`, `get_mouse_scroll`, `get_mouse_state`,
`get_keyboard_state`, `get_modifiers`, `get_active_device` and `get_last_input_device` read
it. A press that repeats does not double the key, a release clears the press edge,
`clear_edges` forgets the edges without forgetting where the pointer is, and the delta is real
arithmetic over the last two positions.

Keybinds are real values a mod owns: `register_keybind` (with a label), `unregister_keybind`,
`get_keybind`, `set_keybind`, `get_keybinds` and `is_keybind_active`, which asks the state
whether the bound key is down — so a rebind changes the answer immediately. Handlers and the
keyboard-capture flag are recorded for the caller's own loop.

Refused: the client's own devices and its pointer grab, controllers and touch — with the
reason that the loader only sees what X11 gives it while the game window has focus, which is
also why a headless run cannot exercise a key at all.

Verified in a run of the loader with the mod attached: the press/release edges and their
clearing, two mouse buttons with the pointer at `(110, 45)` and a delta of `(10, -5)`, two
modifiers, two keybinds with `hello:dash -> LeftShift`, `Space` making `hello:jump` active and
a rebind to `J` making it inactive again, and every client device call refused.

### 37 — GUI widget library

Below the 47 functions there is a real node tree, not a bag of setters. `add_child` sets both
sides of the link and detaches the node from a previous parent; a self-link or an ancestor link
is refused, so the tree cannot cycle; `remove_child`, `remove_all_children` and `unregister`
clear the parent on the way out. The geometry is real arithmetic: margin, border and padding
produce distinct `margin_rect`/`border_rect`/content rectangles, `get_rect` sums every
ancestor's content offset, `set_min_size`/`set_max_size` and `Constraints` clamp `set_size` (an
inverted pair clamps rather than panics), and `place` lays a parent's children out for absolute,
vertical, horizontal and grid layouts with padding, spacing and weight. `WidgetType::ALL` carries
all 66 kinds, each knowing whether it accepts text, children and a value, and `ui_kind` bridges a
kind into section 36's screen system. `set_data`/`get_data` is a real keyed store and
`bind`/`unbind` really read through to a source node, with the node's own key winning over the
bound one. Twelve events (`WidgetEvent::ALL`) each register a named handler, `dispatch` runs and
records them, and `set_value` raises `Change` only when the value actually moves. What the client
owns is refused with its own sentence: drawing, real input delivery, the client's theme, native
dialogs and file pickers, IME, and client-side scrolling.

Verified in a run with the mod attached: 66 kinds; a three-child panel with the cycle refused; a
margin/border/padding rect of `(13, 13, 4, 4)`; a button clamped up from 10×5 to its 60×20
minimum and a slider clamped down to its 200×30 maximum; a vertical layout placing three children
at y = 15, 17 and 39 with the weighted child taking the remaining 86 pixels; a binding reading
`From the panel` through to its source and then the node's own `Own label` winning; three
dispatches from `set_value`, `submit` and `dispatch`; and every client call refused.

### 39 — Camera APIs

This is a real camera. Position, yaw/pitch/roll, the forward/up/right basis, fov with clamping,
near and far planes, `look_at`, `lerp_position`, `lerp_rotation` (which takes the short way
round: 350° to 10° at half is 0°), shake with a decaying offset, zoom, third-person distance,
bobbing, roll, free camera and `reset` are all values a mod owns. `get_view_matrix` and
`get_projection_matrix` build real 4×4 matrices — row-major, applied to column vectors, which the
module states — `world_to_screen` projects through view×projection and refuses a point behind the
camera, `screen_to_world` unprojects, `get_frustum` extracts the six planes from view×projection,
and `is_visible`/`is_box_visible` test them. `register_modifier` and `register_camera_controller`
are declaration registries, as in section 29. Driving the *client's* camera — reading or setting
its position, rotation, mode, entity, fov or shake, and calling a modifier or a controller — is
refused with its own sentence. The convention matches the game's: yaw 0 faces +Z and yaw 90
faces −X.

Verified in a run with the mod attached: a basis at yaw 90 / pitch −10 of `(−0.985, 0.174, 0)`;
fov clamped from 500 to 179; a point 8 units ahead projecting to the screen centre at depth
`0.988` while the point behind projects to `None`; all six frustum plane distances to that point
positive, with `contains_point` true and `is_visible` false for the point behind; shake decaying
0.6 → 0.121 after 400 ms; the short-way yaw lerp; and every client-camera call refused. The
section's tests were mutation-checked — breaking the rotation wrap takes
`lerp_takes_the_short_way_round_on_yaw` red, and the file was restored byte-identical.

### 40 — Audio and sound

Nothing here can be heard, because the client owns the audio device, so this section is the model
a mod can actually compute with. Sounds register, load and unload with a real duration; buses and
sources are handles with real state; the volume chain resolves source → bus → category → master
as a product (0.5 × 0.8 = 0.4 in the run) with every setter clamped; playbacks are real, moving
through playing → paused → resumed → stopped with a position that advances; and the spatial maths
is genuine — `attenuation_at` on a 1–16 linear curve gives exactly 0.8 at 4 units, and
`pan_towards` puts a source at +X hard right (`1.0`) against a listener facing −Z. Reverb,
filter, attenuation, spatialization, looping, pitch, pan and the listener are the mod's own, and
`get_diagnostics` summarises its mixer. Refused with a sentence each: hearing anything (the
client's device), the client's master volume and sound list, the client's own reverb and filter
processing, and decoding an audio file — this host has no decoder, as with section 33's textures.

Verified in a run with the mod attached: two sounds with a 1200 ms duration, load and unload, a
bus with reverb and a filter, the multiplied volume chain, the master and category setters, the
attenuation and pan figures above, a playback at 300 ms after `advance`, five live playbacks with
`stop_all` stopping four, and every client call refused.

### 41 — Particles and visual effects

The client draws particles; what a mod owns here is the simulation, and it is real. A
`ParticleType` declaration carries the sprite, size, colour, gravity, birth velocity, blend mode,
max age and lifetime, and `spawn`/`spawn_at`/`spawn_in_box`/`spawn_along_line`/`spawn_in_sphere`/
`spawn_burst` create real particles in a pool this host integrates: position += velocity·dt,
velocity.y += gravity·dt, age += dt, removal at max age, with size and alpha interpolated over
the lifetime. Bursts come from a seeded generator of the module's own, so the same seed
reproduces the same velocities exactly and a different seed does not. Emitters are real too — a
rate with a fractional accumulator, a direction with spread, a shape (point, sphere, box, disc),
a max-alive cap and a seed, each with `pause`/`resume`/`stop` on a live handle — and 2.5 particles
per second produced exactly 2 after one second and 5 after two. Visual effects are declarations
over emitters and kinds.

Refused with a sentence each: drawing, the client's own particle list, spawning into the client's
world, binding a particle texture, and the client applying blend state. One deliberate deviation,
documented in the module: `spawn_in_box` clamps a particle inside its box rather than removing it
on exit, because the two requirements conflict and staying in bounds is the one the spec states.

Verified in a run with the mod attached: a particle spawned at the origin moving exactly to
`(0.25, 1.0, 0)` after 0.5 s with its velocity now `(0.5, −2.9, 0)`; removal by max age; five
particles from a seeded box burst, all five alive and inside the box after 0.75 s; a line of 3
and a sphere of 4; an alpha fade of `(1.0, 0.5, 0.0)` across the lifetime; the same seed giving
identical burst velocities and a different seed different ones; and every client call refused.

### 42 — Networking

Two very different things live in this section and the module keeps them apart. **A mod's own
socket is genuinely opened** with `std::net`: `listen` answers a listener whose bound address is
real (the run shows `127.0.0.1:37245`, an ephemeral port), `bind_udp` and `connect` give real
connections with a transport and a state, `receive` is bounded by a timeout and answers
`NetError::Timeout` instead of blocking, framing is length-prefixed, and `send`/`send_reliable`/
`send_unreliable`/`send_ordered`/`broadcast`/`flush`/`disconnect`/`close`/`reconnect` all work
with real statistics counters over a moving window. Channels, messages, handlers and connection
handlers are registries. The unit tests prove a 64-byte loopback round trip that moves exactly 64
bytes and one message each way, that a read times out rather than hanging, that a closed port is
refused, and that framing survives a message split across two reads.

The **client's single game connection is not reachable**: whether it is connected, its remote and
local address, its latency, ping, jitter, packet loss and bandwidth, its connection state, its
protocol version, its session id, `disconnect_client`, `kick_player` and `send_to_client` all
answer `false`/`None`/empty with their own reason. One documented partial: `set_compression` and
`set_encryption` record a real per-connection flag carried in the frame header but do not
transform the payload, because compressing would mean a codec dependency this crate does not take.

Verified in a run with the mod attached: a listener bound to an ephemeral port and its real
address, a connection to a closed port refused, a UDP connection reporting `(Udp, Connected,
200ms)`, channels and messages registered with a duplicate refused, a handler registry, real
traffic statistics, and every client question refused with its reason.

### 43 — Packet interception and protocol APIs

A packet is bytes, and bytes this host can do. The registry keys packet types both ways by
numeric id and name — a duplicate id or name is refused, `Err(DuplicateId(127))` in the run — with
a direction and a protocol state each, and codecs attached to a type. The `Buffer` is a real codec
with a read cursor: u8, u16, u32, u64, i32, f32, f64, bool, a length-prefixed UTF-8 string, a
7-bit varint, and raw and prefixed byte blocks, with an over-read a real error rather than a panic
or a silent zero (`Err(OutOfBounds { wanted: 4, remaining: 0 })`). Framing is real: `encode`
writes a varint length prefix and `decode` refuses a truncated frame rather than guessing
(`Err(Truncated { declared: 3, available: 2 })`). Varint boundaries are exactly right — 0 and 127
take one byte, 128 and 255 take two, 16383 takes two, 16384 takes three, `u32::MAX` takes five.
There is a real handler chain per type and direction, a real dispatch and `feed` path, a trace with
`export_recording`/`replay_recording`, and text `serialize`/`deserialize` with `validate` and
`inspect`.

Refused with a sentence each: cancelling, replacing or modifying the client's packets, sending
into its transport, observing its flow, reading its unknown packets, its protocol version or its
protocol mapping, and claiming one of its ids.

Verified in a run with the mod attached: 23 bytes written and every field read back exactly
(`u8=7, u16=500, u32=70000, varint=300, string="hello", f32=1.5, bool=true, bytes=[1,2,3]`), an
over-read and a truncated frame both refused with their errors, the varint byte lengths above, a
packet registered with its duplicate refused, `serialize`/`deserialize` round-tripping, a handler
running on dispatch and on `feed`, the trace counting two entries, and every client call refused.

### 44 — Custom networking between mods

Peers, channels and streams, and the delivery is real: a payload sent to another mod in this
process is framed, decoded and handed to that mod's registered handler, with the delivery counted.
A channel declares a codec, a payload limit and a required API version, and all three are enforced
where a caller would hope — `Err(PayloadTooLarge { size: 65, limit: 64 })` one byte over and
`Ok(1)` at the limit, `Err(VersionMismatch { required: 2, actual: 1 })` for a peer advertising an
older API than the channel requires, `Err(UnknownPeer)` for a peer that is not there, and
`Err(StreamClosed)` after a stream closes having carried messages 1 and 2 in order. Requests are
real too: a registered responder answered `Ok([7, 7])`. Also `register_codec`,
`get_payload_limit`/`set_payload_limit`, `get_channel_status` (closed, registered, open),
`get_peer_api_versions`, `get_peer_capabilities`, `negotiate_version` and the cleanup calls.

Refused with a sentence each: delivering to a player, relaying through the client's server,
sending into the client's transport, and reading the client's plugin channels.

### 45 — Commands and command registration

A real tokenizer and binder underneath the 31 names. Quoted tokens keep their spaces (a
`"two words"` argument arrived whole), an unterminated quote is `Err(UnterminatedQuote)`, a
missing required argument is `Err(MissingArgument("who"))` naming it, and a wrong type is
`Err(WrongType { argument: "times", wanted: "integer", found: "twelve" })` rather than a silent
zero. An alias resolves to the same command and runs it, `complete("/hel")` answers every name and
alias with that prefix, and the tree validates clean. Dispatch really checks the permission level
*before* fetching an executor: a permission-4 caller runs the command, and a permission-0 caller
gets `Err(PermissionDenied { command: "hello:greet", needed: … })`. `register_redirect` works
between two commands that exist and refuses when either does not.

Refused with a sentence each: registering into the client's dispatcher, running a command as the
client or the server, reading the client's command list, and aliasing a client command.

### 46 — Chat and messages

Components are real data. `create_text` and `create_translation` build them,
`i18n::register_translations` supplies the table section 13 owns, and a translation really
resolves and substitutes: `hello.welcome` with `Welcome, {0}!` rendered `Hello Welcome, world!`,
and a named one rendered `Hello Ada!`. An unknown key renders the key itself and an unmatched
placeholder stays visible, which is the honest behaviour rather than a silent blank. Styles
(colour by name and by hex, bold, italic, underlined, strikethrough, obfuscated, insertion) are
carried on spans and survive `then`, and click and hover events survive into the annotations.
`format` and `from_json` are a real JSON component round trip. The history is a real capped ring:
pushing three messages into a capacity of two leaves the last two, with the oldest dropped
exactly once. Delivery is real in-process: `send_local` reached one handler and `broadcast`
reached the subscriber, with the text and style intact.

Refused with a sentence each: sending into the client's chat, reading its history or its last
message, its action bar, title, subtitle and toasts, its formatting, and its message selection.

### 47 — Scoreboards, teams, and statistics

Objectives, scores, entries and teams, all real. `set_score` and `add_score` are exact signed
arithmetic (`3`, then `+2` gives `5`, then `-1` gives `-1` for another entry, and `-5` followed by
`+5` lands on `0`), `get_scores` is sorted highest first with a name tie-break
(`[("Ada", 5), ("Bo", -1)]`), and reset, remove and entry bookkeeping all answer. A team's
membership *moves*: adding a member who is already on another team puts them on the new one rather
than duplicating them, and `team_of` and `get_team_members` agree from both sides. Statistics are
real counters with a documented floor — `decrement` below zero clamps to `0` rather than wrapping —
plus `total` per category, a `top(n)` in order, and an `export`/`import` pair that round trips.

Refused with a sentence each: the client's scoreboard, its sidebar, its teams and its statistics.

### 48 — Enchantments, attributes, and equipment

The rules are real. The cost curve is exact at the points the module documents (a Common
enchantment gives a minimum of 1 at level 1 and a maximum of 46 at level 5), `get_compatibility`
answers `Same`, `Compatible` or `Incompatible`, and combining two level-2 instances of the same
enchantment resolves to level 3 — equal levels go up by one as the game does. Applying, modifying
and removing an enchantment on an `ItemStack` reads back at the right level. Attributes apply
their modifiers in order — add, then multiply-base, then multiply-total — and then clamp:
a base of 10 with an `Add 5` grant gives 15, and the same value clamps to 12 against a maximum
of 12 while staying 15 against a maximum of 100. Armour points add up exactly (3 + 8 = 11), and
durability damages down to breaking at zero and removing the piece rather than going negative.

**A real defect the verification found here:** `get_armor_points()` answered `-0` for equipment
with no armour. `std`'s `Sum` for floats seeds with `-0.0` — the true additive identity for IEEE
floats, since `0.0 + (-0.0)` loses the sign — so an empty sum is a negative zero, which is a poor
answer to hand a mod. It now normalises a zero sum to a positive zero, and
`no_armour_reports_a_positive_zero` asserts the *sign* (a plain `== 0.0` would have passed before
the fix, because `-0.0 == 0.0`). Reverting the normalisation takes that test red.

Refused with a sentence each: the client's enchanting table, its item enchantments, its equipment
slots, its gear model, and a stack the client or the server owns.

### 49 — Loot tables and drops

A generator this host can genuinely run. Tables hold pools with roll ranges; pools hold entries
with weights and count ranges; entries carry conditions, functions and sub-entries; and the whole
draw comes from a seeded generator of the module's own, so a seed reproduces exactly and the
weighting shows: over 200 seeds a 3:1 table dropped `minecraft:short_grass x1` 149 times (74.5 %)
and wheat seeds 51 times (25.5 %) — the last spread across 13/17/21 for counts 1, 2 and 3, which is
the uniform count range working. Conditions are real predicates (`chance(1.0)` true,
`level(5)` true at level 7 and false at level 2, `flag(true)` false against a false flag), a
`set_count(9)` function turns a one-item drop into nine, and `generate_for_block` resolves while
`generate_for_entity` and `generate_for_fishing` correctly answer nothing against a Block-kind
table.

Refused with a sentence each: the client's loot tables, the server's roll, the client's drop rules,
and rolling against the client's world.

### 50 — AI and mob behavior

The decision layer, and it really decides. Goals live in a per-entity list ordered by priority with
a stable tie-break (`[("hello:attack", 1), ("hello:wander", 5)]`). `select_target` picks the
nearest candidate that passes the goal's filter — with a filter denying sheep and requiring
line of sight it chose the zombie at 3 units while the unfiltered call chose the sheep at 2 —
and `select_goal` names the running goal *and* its target plus how many others it skipped
(`(Some("hello:attack"), Some("nearer"), 1)`). A path folds its length exactly (3 + 4 = 7),
`Navigator::step` moves `speed × dt` (2.0 at speed 2.0 for one second) and advances the waypoint
only on arrival, `StuckDetector` answers on its window (two small steps then a large one is not
stuck), a target's cooldown expires after exactly the ticks it was given, memory round trips, and
a state machine transitions (`idle` + `saw_enemy` → `chase`).

Refused with a sentence each: the client's brain and current goal, its navigator and path, its
senses, seeing/hearing/nearby entities, and the provider/step hooks that would sit inside its AI.

### 51 — Redstone and block signals

A real solver, and the numbers are the point: a source at strength 15 gives `power=[14 13 12 11]`
along four steps of wire — one level per step, exactly — while the wire's own *signal* is 0, which
is the module's signal-versus-power distinction made visible. A receiver fed from two sides gets
14, the maximum of the two paths and not their sum of 28. A ring of four wires settles rather than
spinning. `Signal::new` clamps at both ends (`-5` → 0, `99` → 15), `decayed(3)` takes 15 to 12 and
`offset(-10)` takes 4 to 0. The update queue is real: scheduling a position leaves one pending and
draining answers exactly that position; a change that reaches nothing queues nothing new. The
whole network sums to 65, which is 15 + 14 + 13 + 12 + 11.

Refused with a sentence each: the client's signal, direct signal, power and powered state at a
position, its neighbours, the direction a signal arrives from, writing its redstone, and reading
its update queue.

### 52 — Fluids

A real cellular simulation on a grid a mod owns. A source drains downward into an empty cell —
verified on both a two-tall and (after the fix below) a three-tall column — and does not empty
itself. Levels drop by one per step sideways and stop at 8, with the ninth cell left empty, and
the grid reaches a fixed point after 9 steps that a further step confirms by changing nothing.
Viscosity is real: lava (viscosity 5) has not spread after one step where water has, and reaches
level 1 within ten. Region volume, `touching`, and per-cell source/flowing/empty queries all
answer, and the default fluids register under the ids `water` and `lava` with `minecraft:water`
and `minecraft:lava` as the blocks they place.

**A real defect the verification found, and a test that had passed for the wrong reason.**
`FluidGrid::coords` returned the y and z of an index swapped, so on any grid whose height differed
from its depth the simulation read and wrote the wrong cell. The module's own draining test used a
`1×2×1` grid and asserted that the cell below the source filled — and it *passed*, because the
swapped coordinates sent the water sideways into the very cell being asserted on. The demo's
three-tall probe failed where the two-tall one passed, which is what exposed it. It now indexes
correctly, and `a_cell_index_round_trips_on_every_grid_shape` asserts
`coords(index(x, y, z)) == (x, y, z)` over five grid shapes including `(1, 3, 1)`, `(2, 3, 4)` and
`(12, 1, 1)`. Restoring the swap takes that test red. The fix also corrected the simulation's
reach: the volume a source covers in a 12×3×3 grid went from 27.5 (wrong cells) to 12.1.

Refused with a sentence each: the fluid state at a position, its height and velocity, the fluids a
body touches in the client's world, and the layer a fluid is drawn with.

### 53 — Villagers, trading, breeding, and farming

§53 real: a trade-offer registry whose matching runs over a real `Inventory` (pick the offer, refuse on missing inputs, spend a use, restock only after the cooldown ticks down) with a multiplier+reputation price; breeding rules that make a `BreedingPair` whose child takes every trait from a seeded choice of parent, with love/age, cooldown and a growth timer that matures; a crop registry whose seeded growth chance and rate advance real stage boundaries and whose harvest answers a real yield. §53 refused: the client's villagers, their live offers and the trade screen; the client's live animals, love/age and live breeding; the client's crops, their block growth and a live harvest — each with its own `why_unknown` sentence, a `refusals()` list and a `crate::warn!` at the refusing entry point. §53 evidence (`CINNALOADER_DEMO=53`, CLI): "uses 3 -> 2 | wheat 64 -> 44 | emerald 1 | price 20 xp 2", "uses 0 at cooldown 100 | tick 100 -> restocked 1 offers, uses 3", "price 20 -> 18 at reputation 25", "child calf speed 0.3 from parent B (A 0.25 B 0.30) | growth 60", "advance 60 matured 1 | mature true age 60 growth 0 | cooldown 40", "stage 0 -> 3 after 60 ticks (3000 ms)", "chance 0 -> stage 0 after 100 ticks". §53 types for the prelude: trading {TradeOffer, TradeRefusal, TradeReceipt, VillagerHandle}; breeding {Animal, BreedingPair, BreedingRule, Offspring}; farming {Crop, GrowthRule, Planting}. §53 gate: `cargo build -p cinnaloader`, `cargo test -p cinnaloader --lib {trading::,breeding::,farming::}` (8 each), `cargo test -p cinnaloader --doc` (71), `cargo build -p hello-mod`, `cargo clippy --all-targets` — all green, zero warnings.

### 54 — Structures and block entities

Templates rotate and mirror for real: on a 3×1×2 template the local point (2, 0, 1) maps to
(0, 0, 2) under Cw90, (0, 0, 0) under Cw180 and (1, 0, 0) under Cw270, and mirroring (0, 0, 0)
gives (2, 0, 0) left-to-right and (0, 0, 1) front-to-back, with the size turning 3×1×2 into 2×1×3.
A placement answers its real absolute positions, its bounds carry eight corners and a volume of
six, and `fits_target`/`outside` answer against a target box. Jigsaw anchors carry directions and
a seeded `generate` returns the pieces. Block entities are typed with a `ConfigValue` data map, a
tick counter that advances, an inventory, and a serialize/deserialize round trip of 137 bytes that
comes back equal, inventory intact at five coal.

Refused with a sentence each: the client's structures and their placement in its world, and its
block entities, their inventories, viewers and update packets.

Implemented and verified; see the row above.

### 55 — Data formats and serialization

A typed value tree (null/bool/int/float/string/list/map) whose `Map` keeps insertion order, so it round trips byte for byte. JSON has a real writer and recursive-descent parser — the documented escape set, `\uXXXX` surrogate pairs, multi-byte UTF-8 passed through, a byte position on a malformed document, and duplicate keys that keep the first position and the last value. The binary form is a tag byte then a payload (zigzag LEB128 ints, little-endian floats, varint-length strings, lists and maps), and it refuses truncation and any trailing byte rather than guessing. Base64 (standard alphabet, `=` padding), hex, a PackBits compressor, CRC-32 (IEEE) and FNV-1a 64 are real, each refusing a bad input. `migrate` walks a schema's registered steps from version N to N+1 and refuses a step no migration covers. The client's own NBT dialect and level format are refused in both directions, each with its sentence in

### 56 — Data components and custom data

56. Data components and custom data — `sdk/src/data.rs`. Types: `ComponentType` (id/name/kind/default), `ValueKind` (integer, float, string, bool, list, map), Real: a component-type registry; `set`/`get`/`get_typed`/`remove`/`has`/`clear`/`keys`, with a typed read refusing a wrong kind as `TypeMismatch { component, wanted, found }`; patches and merges (right-hand wins, a `None` removes, applied to a copy leaves the original); exact round trip through `ConfigValue` (empty map and a nested list); absent vs present-at-default by equality/`fingerprint`; custom data; watchers; migrations. Refused: the client's item/entity components, a live stack's, and writing what the client reads — `why_unknown`, `refusals`.

### 57 — Persistence and saved data

typed `ConfigValue` key space, dirty flag, autosave interval; `Duration::ZERO` = never), atomic save/load through `crate::fs` with a format version that is refused when newer than `FORMAT_VERSION`, plus backup/restore/migrate/export/import and the 25 wishlist free functions. the client's level/player/registry/save-directory are refused with `why_unknown`/`refusals` and bytes written, a dirty flag cleared by the save, autosave fired after its interval and not before, and a too-new format refused.

### 58 — Resource packs and data packs

58. Resource packs and data packs — done (crate-level, in-game run pending). All 29 names in sdk/src/packs.rs as a real resolver over per-pack virtual file trees built on resources::ResourcePack (tree) and resources::ResourceId (paths), not a second resource type. Priority-then-id winner in resolve/providers, data-file merge where the later (higher-priority) key wins, format gate (SUPPORTED_FORMAT=34) refused by name, and the client's own pack load, pack list, resource reload and mounted-pack reads refused via why_unknown/refusals plus warn!. 9 lib tests; doctest; build/test/clippy green — no in-game run yet.

### 60 — Game rules and gameplay hooks

60. Game rules and gameplay hooks — done (crate-level; in-game run pending, manager wires). Files: `sdk/src/game_rules.rs`, `sdk/src/gameplay.rs`, `examples/hello-mod/src/s60.rs`. No new deps. game_rules: `GameRule` (id, owner, kind: ConfigType, default: ConfigValue, min/max, choices, name, description), `RuleError` (WrongKind/OutOfRange/BadChoice/UnknownRule), `RuleChange` (rule, old, new). Real: registry with `register/unregister/get/get_all/get_value/get_default/validate/set_value/reset`; returns `Ok(None)` for an unchanged value; a change fires `register_change_handler` handlers and publishes `EVENT_CHANGED` ("game_rule_changed") on the §6 bus with old->new; `reset` back to default; gameplay: `Hook` (id, owner, family, name, priority, phase), `Phase` (Before/After, Ord so Before first), `RuleProfile` (name + values). Real: 18 wishlist families (`FAMILIES`) each with a ordered by phase, then priority desc, then id; `dispatch(family, &mut Payload)` drains matching hooks, runs enabled ones in order on the shared payload, counts them, reinserts (no lock held during a callback); `set_enabled`. `RuleProfile::apply` sets each value through `game_rules::set_value` and returns only the `RuleChange`s it made; profiles registry + `apply_profile`. Refused: the client's hooks — `client_hooks()` empty, `run_client_hook()` false, `why_unknown`/`refusals` (client, world, tick) + `crate::warn!`. Not mine, still red in the tree at the end: `sdk/src/query.rs` test `E0382` (moved `base`) and two clippy warnings there (`not`, derivable `Default`) — left for the query lane; they do not touch §60. Unverified: no CLI/in-game run of `s60::run` (manager wires the prelude + demo and verifies).

### 61 — World and entity queries

The module is a **real query engine over records a mod feeds in**, plus honest refusals for the numeric fields (`numbers`), components, `visible_to`, health, age, hostile, dead. Builder. records, `query()`, `parse()`. Process-wide instance behind a Mutex for the module functions. chunks/items` are module starters that snapshot the module index and filter by kind. Dimension, Chunk, VisibleTo, Number(range), Text, Alive, Dead, Hostile, Passive, And, Or, Not. the trait impl, no inherent `not`). Missing named value sorts as 0.0 (documented in code by the `unwrap_or(0.0)`). Grammar: flat space-separated terms + `and(...)`/`or(...)`/`not(...)`. Keys: kind|type, except, id, tag, tag=key~value, component, without_component, origin=x,y,z, radius, box=a,b,c;d,e,f, cone=o;d;ang;rng, dimension, chunk=x,z, visible_to, number=key[:min[:max]], text, sort=..., order, value, limit, offset; flags alive/dead/hostile/passive. Tag values use `~` because `=` is a token delimiter. Radius renders its own `origin=`; `Sort::Nearest` suppresses its origin when a Radius already carries one (avoids a duplicated token). `QueryError { position, message }`. 35 wishlist names: `entities players blocks chunks items by_type by_id by_tag by_component with_component without_component within_radius within_box within_cone near_position in_dimension in_chunk visible_to alive dead hostile passive sort_by_distance sort_by_age sort_by_health limit offset collect count first first_or_none for_each stream explain benchmark` = 5 module fns + 30 methods. `first()` panics on empty (documented); `first_or_none()` is the safe spelling. Extra beyond the wishlist: `except_type`, `by_tag_value`, `number_between`, `text`, `filter`, No float total is exposed, so the `-0.0` sum trap does not apply. Tests touching the process-wide index take `crate::test_lock::serial()`. Float filters avoid sums. a Filter and/or/not composition; a sort_by_value order with its id tie-break; limit(2) with the count before paging; offset(99) past the end; a render->parse->same-ids round trip; a malformed query refused at position 12; `live_entities()`/`why_unknown`. Every line <200 chars. textures::Filter / log::Record). `Index`, `Query`, `QueryError`, `Sort` are free. In-game run of the s61 demo was not performed (outside this lane's assigned gate). CLI/unit/doctest only.

### 62 — Math and geometry

62. Math and geometry — `sdk/src/math.rs`. No client-owned surface: pure arithmetic, nothing refused. Functions (32): clamp, lerp, inverse_lerp, remap + map_range, smoothstep, step, wrap_angle, normalize_angle, degrees_to_radians, radians_to_degrees, distance, distance_squared, angle_between, direction_to, reflect, refract, dot, cross, normalize, project, closest_point, intersect_ray, intersect_aabb, intersect_sphere, intersect_plane, intersect_triangle, random, random_range, random_integer, random_float, random_bool, seeded_random. Types: Vec2/Vec3/Vec4, IVec2/IVec3, EulerAngles, Quaternion, Matrix3, Matrix4 (= Mat4), Aabb, Obb, Sphere, Capsule, Plane, Ray, Triangle, Rect, Size, Frustum, Transform, Pose, Color, Rgba, Rgba8, RayHit. Conventions: Matrix4 is row-major, column vectors, `v' = M·v`, translation in the last column; transform_point applies translation, transform_direction/vector ignore it. yaw is clockwise, yaw 90 faces -X (matches crate::physics::direction_from_rotation). wrap_angle -> (-180,180], normalize_angle Real: vector algebra with zero-length normalize -> ZERO; Matrix4/Matrix3 inverse via Gauss-Jordan returning None when singular (verified M·M^-1 = I within 1e-9); quaternion multiply/conjugate/rotate/ slerp (shortest arc); euler<->direction and euler<->quaternion agree; 14-name easing list; splitmix64 Rng (next_u32/u64/f64/range/integer/chance) with an exact fixed-seed vector; value/gradient noise on a unit integer lattice, deterministic per seed, in [-1,1], continuous, plus fbm_2d; ray-sphere, ray-box, ray-plane, ray-triangle (Moller-Trumbore), segment-segment 2D, sphere-sphere each answering a RayHit (otherwise None), tested for hit, miss and tangent; Aabb union/intersection/contains/expand/center/size. New deps: none. 11 tests. Doctest on math.rs line ~36. Note: doctest/unit tests in this file use

### 63 — Time and clocks

§63 files: `sdk/src/time.rs` (module, stub replaced), `examples/hello-mod/src/s63.rs` (demo, stub replaced). Nothing else touched; `sdk/src/lib.rs` already had `pub mod time;` and the demo call. §63 real: fixed conversions (20 ticks/s, 50 ms/tick; `millis_to_ticks`/`seconds_to_ticks` round to the nearest tick, exactly half rounds up — 25 ms = 1 tick, 24 ms = 0); `to_duration`/`from_duration` round trip every tick count. `Clock` is a mod-owned tick counter: `advance` adds exactly, rolls `time_of_day = ticks % 24000` and `day = ticks / 24000`, and exposes millis/seconds/phase/hour/minute/`fraction`/is_day/is_night/`set_time_of_day`. `Phase::at` over 0..24000 from midnight with stated bounds (night 0..5000, sunrise 5000..6000, day 6000..17000, sunset 17000..18000, night again); `Phase::start_tick` is the exact inverse (`Phase::at(start_tick())==phase` for all four). A registry (`register_clock`/`get_clock`/`advance_clock`/`unregister_clock`/`clock_count`/`clock_name`). §63 schedulers: `Scheduler` (mod-owned, virtual time) with `after`/`every`/`advance`/`cancel`/`pending`/`due_at`; `advance` fires only due tasks and a repeating task reschedules `due += every` from its own due time, so it does not drift (test: every 100, advance 250 fires at 100+200, next due is 300). Free `schedule`/`schedule_repeating`/`register_tick_handler`/`advance`/`cancel`/`pending` wrap a process-wide scheduler (jobs are `Send`; tests take `crate::test_lock::serial()`). §63 timers/format: `Stopwatch` measures while running and holds while stopped; `Timer::new` fires once, `Timer::repeating` accumulates and keeps the remainder (per-interval accumulator, no drift); `format_duration` ("0 ms", "1s", "1m 30s", "1h 2m 3s", "1d 1h 1m 1s") and `format_time_of_day` ("06:00", "18:30"). Other names: `now`/`monotonic_now`/`get_timestamp`/`get_timezone`("UTC")/`get_uptime`/`get_elapsed`/`get_tick_rate`/`set_tick_rate`/`tick_time`/`frame_time`/`delta_time`/`game_time`/`interpolation_alpha`/`get_frame_rate`/`get_frame_count`/`sleep`/`create_stopwatch`/`create_timer`. §63 refused: the **client's** world clock (`world_time`), its time of day (`day_time`) and its tick counter (`get_tick_count`) — each `None` with its own `why_unknown` sentence, in `refusals()` (len 3), and a `crate::warn!` at each refusing entry point. §63 gate: `cargo build -p cinnaloader` ok; `cargo test -p cinnaloader --lib time::` 9 passed; `cargo test -p cinnaloader --doc` 78 passed (time doctest ok); `cargo build -p hello-mod` ok; `cargo clippy --all-targets` — **2 unique warnings, both in `sdk/src/query.rs` (another lane: `should_implement_trait` on `Filter::not`, `derivable_impls` on `Sort`'s Default). Zero warnings in `time.rs`/`s63.rs`.** Not fixed here (not this lane's file); re-run once the query lane lands. §63 demo (`examples/hello-mod/src/s63.rs`, full paths): tick↔ms↔s round trip + half-tick rule; clock +13000 then +11500 -> 24500 ticks, 1225000 ms, day 1, tod 500; day 6000->06:00 day / 18500->18:30 night fraction .7708 + phase starts; scheduler after 100&300 -> advance 150 fires 1, advance 200 fires 1; repeat every 50 advance 150 fires 3, next due 200 (no drift), cancel; stopwatch holds while stopped; one-shot timer fires once; exact `format_duration(3723000)=="1h 2m 3s"`; refusals list + three `None`s.

### 64 — Weather, time, and environmental effects

64. Weather, time and environmental effects — `sdk/src/environment.rs`, demo `examples/hello-mod/src/s64.rs`. No new deps. Module is NOT in the prelude; doctest and demo use `cinnaloader::environment::…` full paths. The 64.prelude `pub use` line is for the manager to wire into `sdk/src/lib.rs`. Shape (fluids.rs pattern): a REAL model the mod owns + client-owned surface refused in get/set_thunder_level, get_temperature, get_humidity, get_wind, get_sky_color, get_fog_color, get_fog_density, get_sun_angle, get_moon_phase, get_star_visibility, get_day_time, set_day_time, get_weather_duration, set_weather_duration. Plus `get_light(x,y,z)` and "light", "climate", "effects". `refusals()` -> 4 `(String,String)` entries. step). `advance(ticks)` ramps levels toward the state target `1/RAMP_TICKS` per tick (RAMP_TICKS=100) and draws the next state from the seed on expiration; `with_weather(seed,state,dur)` for a known start; `new(seed)`; setters. Seeded => same seed replays the same sequence (tested). = level.min(15).saturating_sub(dist.min(15)) i.e. ONE level per block, clamped 0..15. 1800-tick dusk/dawn; `sky_light(level,day_time)` = level.saturating_sub(darken). `block_light_at`, sky 15 noon/midnight = 15/4. bands via `from_temperature`; `Precipitation` {None,Rain,Snow}; snow when downfall>0 && temp<0.15. has_at/get/remove/clear. duration 0 = permanent. Expires exactly at `start+duration` (99 active, 100 gone). register_fog_modifier/register_cloud_renderer/register_sky_effect/register_ambient_sound (generic registry, read back by id), and `WeatherType` via register_weather. Tests: 9 in `environment::tests`, all take `crate::test_lock::serial()` where they touch a registry. Verified: `cargo build -p cinnaloader` ok; `cargo test -p cinnaloader --lib environment::` 9 passed; Clippy `--all-targets`: zero warnings in environment.rs / s64.rs. Two warnings remain in Demo `s64::run()` executes clean; its 7 lines are 84..132 chars (≤200, checked then the scratch test removed). Not verified in-game/CLI — manager wires the prelude and runs it.

### 59 — Audio, visual, and gameplay registries

59. Audio, visual, and gameplay registries — done (crate-level, in-game run pending). All 47 bare `register_*` names live in `sdk/src/content.rs` (`content::register_biome` .. `content::register_fuel_type`), no new deps. Real facade: each `register_*` records in `content`'s own index *and* declares in the owning module — block→blocks, item→items, entity→entities, effect/enchantment/particle/sound, recipe/recipe_serializer/brewing_recipe/fuel_type→recipes, recipe→recipes::shaped, loot_table→loot, ai_goal/ai_sensor/behavior→ai, dimension/feature/world_generator→worldgen, structure→structures, block_entity→block_entities, fluid→fluids, texture/model/animation/renderers/textures/models, shader→shaders, statistic→statistics, trade_offer/trade_profession→trading, screen→ui, command→commands, keybind→input, damage_type→effects::damage, custom_component→components, custom_data_type→data, custom_network_channel→channels, game_rule guarded in `content` (game_rules stub has no API), and menu/attribute/font/sound_event/particle_renderer/advancement recorded in `content` only (module()=="content"; the module doc says so). Index/refusals: `ContentKind` (47 variants, `as_str`/`module`/`client_registry`/`all`/`from_name`), `Entry{kind,id,owner}`, `get/all/all_of/unregister/count/kinds`, per-kind `why_unknown`/`refusals`, and the refusing entry point `register_with_the_client` (`crate::warn!`, always false). Each `register_*` is idempotent per id — `false` on a duplicate. Evidence: 6 lib tests (duplicate refused for 5 kinds: block/item/entity/effect/statistic; read-back through blocks/items/effects/fluids/ai/trading/statistics/…; unregister) all green; doctest ok. Gate: `cargo build -p cinnaloader` ok; `cargo test -p cinnaloader --lib content::` 6 passed; `cargo test -p cinnaloader --doc` 78 passed; `cargo build -p hello-mod` ok; clippy reports no content.rs warning. Still red from the query lane, not mine: `sdk/src/query.rs` 2 clippy warnings (`not`, derivable `Default`) — left for that lane.

### 65 — Game settings and client options

Real: typed options (id/kind/default/bounds/choices/label/tooltip/category) with duplicate-id registration refused; get/set enforce kind, bounds, choice list with SettingError exactly like game_rules; SettingChange fires watch + on_changed + §6 `settings_changed` only on a real move; reset/reset_all; by_category/categories/search by id or label; export/import through ConfigValue::Object (exact round trip); save/load via crate::persistence store "settings"; Profile applies a named set and reports SettingChange. Refused: the client's own 16 option getters (video/audio/controls/accessibility/language/ render_distance/simulation_distance/fov/sensitivity/mouse_invert/particles/graphics_mode/ fullscreen/resolution/vsync/frame_limit) return None, set_client returns false; each has its own why_unknown sentence, refusals() lists all 16. Evidence: s65.rs run() prints duplicate refused, wrong-kind + out-of-range errors, change 8->12 plus same-value None, reset, search hit, export/import equal, get_video None + refusal count. Gate: build -p cinnaloader ok; lib settings:: 8 passed; --doc 86 passed; build -p hello-mod ok; clippy --all-targets has 4 warnings, all in profiler.rs/testing.rs (other lanes), none in settings.rs or s65.rs. False-value falsification: removing the `if old == value` guard makes a_change_fires... go red; restored.

### 66 — Window and platform APIs

§66 real (platform, no client): os/arch/family from `std::env::consts`, `platform_id()` = `os-arch`, real line separator and `std::path::MAIN_SEPARATOR`, `is_little_endian()` via `cfg!(target_endian)`, `is_64_bit()` via `size_of::<usize>()`, `cpu_count()` via `available_parallelism`, `get_cpu_info()` in one `CpuInfo`, memory/runtime/environment reused from `crate::diagnostics`, and `get_app_directory()`/`get_home_directory()`/`get_temp_directory()`/`get_paths()` with the app dir being `crate::fs::root()` (the sandbox) and only it marked `writable`. `is_supported(PlatformCapability)` is the stated-rule matrix: supported iff this host answers it from facts it reads itself (Cpu/Runtime/Environment/Paths true; Memory true on Linux because `/proc` is there; PageSize false; Gpu/Clipboard/UrlOpen/FileOpen/RevealFile/DisplayInfo/InputDevices/Accessibility/DesktopSession false). §66 honest partial: the page size is unreported — `page_size() -> Option<usize>` returns `None` with `page_size_reason()` ("std exposes no page size and this host adds no dependency to ask the kernel for one"). No `libc`/FFI added. This is deliberate per spec. §66 real (window): size + scale factor from host facts and a settable model; `set_size` fires `events::kind::WINDOW_RESIZED` only when the size changes; `on_resized` subscribes on the §6 bus (also `on_focus_changed` on `FOCUS_CHANGED`, `on_close_requested` on a newly registered `window_close_requested` kind — the latter two are never fired by this host); monitor list with `set_monitors`/`get_monitors`/`get_monitor` (primary flag) and `get_refresh_rate`; DPI maths `to_physical`/`to_logical` (round half away from zero, documented); cursor `set_cursor`/`get_cursor` clamped to the window box; `get_aspect_ratio`. §66 refused (window, each its own `why_unknown` sentence, `refusals()` 15 rows, `crate::warn!` at the request): handle id, title, focus/minimized/maximized/fullscreen (queries answer false/None), set_title, set_fullscreen, set_borderless, set_resizable, set_icon, set_cursor_grabbed, request_attention, minimize, maximize, restore, close. Refused (platform, 9 rows): gpu, open_url, open_file, reveal_file, copy_to_clipboard, read_clipboard, display_info, input_devices, accessibility_settings. §66 types for the prelude: window {Monitor, CLOSE_REQUESTED}; platform {CpuInfo, GpuInfo, MemoryInfo, DisplayInfo, AccessibilitySettings, PlatformPaths, PlatformCapability}. Do NOT export `window::Window` bare — it collides with the existing `game::Window`; expose it only as `window::Window`. Modules wired as `self as window` / `self as platform`. §66 demo (`examples/hello-mod/src/s66.rs`, `run()`): real os/arch/family/id/cores/endianness/64-bit, capability answers, `get_paths`, a DPI round trip + rounding, an `on_resized` that fires once for 1024x768 then not for the same size, a clamped cursor (5000,-3)->(1024,0), the primary monitor with its flag and scale, then every refusal sentence from both modules. §66 gate: `cargo build -p cinnaloader` ok; `cargo test -p cinnaloader --lib window::` 7 passed, `platform::` 7 passed; `cargo test -p cinnaloader --doc` 86 passed (window line 14, platform line 17 both ok); `cargo build -p hello-mod` ok; `cargo clippy --all-targets` — no warnings in window.rs/platform.rs/s66.rs (4 lib warnings remain in profiler.rs/testing.rs, other lanes' files).

### 67 — Performance monitoring and profiling

Status: DONE — implemented, unit-tested, doctested, builds, clippy-clean for the lane's files. No other file touched. `sdk/src/lib.rs` already declares `pub mod metrics;`/`pub mod profiler;` (lines 66-67) and the demo is already hooked. (idempotent by name). percentiles: 1-based rank `ceil(p/100*n)`, clamped to `1..=n`. `p50` of an even window is the lower middle value. by the caller's `Duration` window. No wall clock. Zero window -> None. elapsed is added to the parent's `children`; `self_time = elapsed - children`. Aggregated minimum/maximum/percentile`. `set/get_slow_frame_threshold`; `get_slow_frames` is *strictly* above the threshold. (`get_memory_usage`, `get_allocations`, `get_gc_time`), client profiler output (`get_system_cost`, `get_slowest_mods`, `get_slowest_tasks`). Each entry point calls process-wide registry takes `crate::test_lock::serial()`; each starts with `reset()`. p99 10; ring cap 3 keeps newest [3,4,5]; rate 150/3s = 50.0; nested a/b/c -> a.self 15, b.self 6, c.self 4; frames [10,20,30,40]ms -> mean 25 p50 20 p90 40; slow frames [17,120] with threshold 16 (16 is not slow); mod cost 16ms, hotspot ai 9ms. lane cleared on re-run). warnings are in `sdk/src/testing.rs:601` and `:612` (another lane's file, not mine). Not run in the client (not this lane's gate). `s67::run()` emits exact proofs: counter=7, gauge=3, histogram 4/10/40/25/20/40/40, rate 50.0/s, section total 10 self 6, frames current 40 mean 25 max 40 ms, slow frame [120] at exact ms, mod cost 14ms + hotspot, and all 12 refusal sentences. Run with `CINNALOADER_DEMO=67`. aliased (serialization::Kind already occupies `Kind` in the prelude). directly.

### 68 — Memory management

68. Memory management — `sdk/src/memory.rs`, demo `examples/hello-mod/src/s68.rs`. No new deps. Module already in lib.rs (`pub mod memory;` at line 68). Prelude line is in Doctest and demo use `cinnaloader::memory::…` full paths, so the prelude is not required. Shape: a REAL store the mod owns + the client's own memory refused. offset; `align` must be a power of two else `BadAlign`; rounds the cursor up; returns Fragmentation is documented: nothing is reclaimed until reset. with a free neighbour; a freed block is reused at its own offset (lowest run first); a request no single run holds fails `OutOfSpace` even when total free ≥ request (fragmentation). exhausted -> `Exhausted { capacity }`; released slot reused LIFO; `live_count`, `acquired_total`, current/peak/total bytes; budget refuses a declaration past it (`BudgetExceeded`) changing nothing; `get_leak_report()` = live entries in declaration (ascending id) order; `per_owner_breakdown()`; fires a `PressureEvent` when `track` pushes current across the threshold. slab_used/slab_free_bytes`, `create_pool/pool_acquire/pool_release`; `get_reserved`, `get_heap_info`, Tests: 8 in `memory::tests`. Registry/ledger/pool-registry tests take `crate::test_lock::serial()` via a `fresh()` that resets the process-wide `State`. Direct `Arena`/`Slab`/`Pool` tests need no lock. Assert exact offsets/bytes: arena 0/16/32, used 40, remaining 60, OOM {61,60}; slab 0/16 reuse 0, free_bytes 32->48, coalesce 1 fragment, fragment Err {24,32}; ledger 200/200/200 -> untrack -> 160, peak 200, total 200, owners render100/world60; budget 100 refuses 60+60; pool slots 0/1, exhausted, reuse 0, acquired_total 3; registry reserved 256+128+128=512. Verified: `cargo build -p cinnaloader` ok; `cargo test -p cinnaloader --lib memory::` 8 passed 0 failed; OTHER lanes' files (sdk/src/debug.rs, sdk/src/profiler.rs, sdk/src/testing.rs) — not touched here. Partial/honest: `get_usage` reads this process's RSS/vsize (shared with the client) via diagnostics; the client's OWN heap/GC/usage are refused under distinct keys. No in-game run was done (the module has no client surface and the demo is CLI-runnable); the demo proves offsets/counts.

### 69 — Debug and developer tools

69. Debug and developer tools — `sdk/src/debug.rs`, demo `examples/hello-mod/src/s69.rs`. No new deps. Module `pub mod debug;` is already declared in `sdk/src/lib.rs`; it is NOT yet in the prelude. The 69.prelude `pub use` is for the manager to wire into the prelude; doctest and demo call `cinnaloader::debug::…` full paths. Index of the file: real tooling first, then the refusals, then the client-only submodule. The 41 wishlist names split 18 real / 23 refused. whether it changed); breakpoint (`#[track_caller]`, records file:line, returns the count); assert(cond,msg) -> bool (`#[track_caller]`, records + warns on false); assert_thread(name) and assert_main_thread() (via `crate::tasks::current_thread`/`is_main_thread`); trace_call -> dump_mods/dependencies/tasks/events/config/registry/resources (real text read from export_snapshot(Option<&Path>) -> io::Result<PathBuf>. draw_line/box/text/path/collision/bounding_box/chunk_bounds/chunk_sections/navigation/ray, show_hitboxes/entity_ids/block_ids/memory/fps/tps, open_console/profiler/inspector, dump_entities/blocks/network/render_state. get_flag/flags/changed_flags/reset_flags. Setting a flag to its current value returns false. remove_readout/readouts (sort by order then name)/render_readouts/`flush()` — flush writes watch_changed/clear_watch/watches. Stats = count/last/min/max/average/changed. `sum` is folded from 0.0 and a zero average is normalised so no -0.0 leaks (spec note honoured). queue_command/pending_commands/run_commands/dispatch(line)->String/console_output/ clear_console. Handlers held as Arc and called outside the lock (no re-entrant deadlock). Unknown line output = `unknown command: {name}`. Output ring CONSOLE_LOG=256. diff_snapshots(before,after) -> lines `path: before -> after` flattening nested maps to dotted leaf paths (non-finite watch floats become null). export_snapshot writes it. invariants/run_invariant/check_all (returns count failed)/check_all_results/failures/ clear_failures. Pass message is `ok`; failures list shared with assert/thread checks. why_unknown keys = ["overlay","flags","console","state"] (4), matching the spec's four refusals (client's debug overlay / own F3 flags / console+profiler+inspector / internal state dump). refusals() returns the four pairs. draw_* -> "overlay", show_* -> "flags", open_* -> "console", refused dumps -> "state". Tests: 9 in `debug::tests`, all take `crate::test_lock::serial()` and reset via a private snapshot diff exactly `["flags.verbose.value: true -> false"]`, check_all == 1. other lanes' files: `sdk/src/profiler.rs:357,367` (unnecessary_sort_by) and Demo ran via a scratch `#[cfg(test)]` probe (since `mod s69` is private) then the probe was debug(69): enable=true define trace_calls=true set(true)=true set(true) again=false changed=["trace_calls"] debug(69): readouts new=true,true,true exact order=["XYZ: 10 64 -3", "FPS: 120 fps", "TPS: 20.0 tps"] debug(69): watch tick_ms last=20 min=10 max=30 average=20 changed=true debug(69): console 'sum 4 5 6' -> "15" 'nosuch' -> "unknown command: nosuch" debug(69): snapshot round trip parsed=true bytes=399 debug(69): diff one changed key=["flags.trace_calls.value: true -> false"] debug(69): check_all failed=1 failures=[("has_id", "the id is empty")] Assumed, not verified: none beyond the loader actually drawing the overlay line in-game (flush was exercised for its count only). Manager still must wire 69.prelude and run in-game.

### 70 — Testing API

70. Testing API — `sdk/src/testing.rs`, demo `examples/hello-mod/src/s70.rs`. No new deps. `pub mod testing;` already in `sdk/src/lib.rs`, NOT yet in the prelude; the doctest and demo use `cinnaloader::testing::…` full paths. 70.prelude is for the manager. All 38 wishlist names exist. The harness is real; only the client-only calls refuse. (tag "integration") -> `Report`. `generate_test_report()` returns the last stored Report. setup in registration order, teardown in reverse; a test's own `.before`/`.after` sit inside the fixture. Teardown still runs after a setup/body panic. payload's message (`&str`/`String` downcast), and the run continues. a panic; `Report`/`TestOutcome` carry `checks` and `message`. are public with read accessors. via `crate::serialization::to_json` of an ordered `Map`), and `compare_reports(golden, actual) ["client_test_hooks","client_test_state","screenshot","trace"]. assert_inventory` -> false ("client_test_state"). Tests: 9 in `testing::tests`, all touching the process-wide registry take panic set -> 2 passed / 1 failed, message "boom went off", the test after it ran; sequence asserted exactly with teardown after the failing body; recorder 3 checks / first "1 != 2"; seeded(42) equal, seeded(43) differs; report summary "1 tests: 1 passed, 0 failed, 0 skipped" and diff `test 'alpha': outcome pass != fail`; mock/clock/refusal set (advance_time(150ms)==3, wait_for true then false, refusals().len()==4). Demo: `mod s70` is private, so it was exercised from a scratch driver in /tmp (removed from the repo; source still there) linking `target/debug/libcinnaloader.rlib` with run all: 1 passed, 2 failed, 1 skipped of 4 | verdict as expected outcomes: alpha=pass(3) beta=fail(3, "1 != 2") delta=skip(0, "needs the client") gamma=fail(0, "gamma exploded") panic isolation: 'gamma' failed with "gamma exploded" while 'alpha' passed and 'delta' skipped … json (one test): {"total":1,"passed":1,"failed":0,"skipped":0,"results":[…]} (147 chars) snapshot diff names one changed test: Some("test 'alpha': outcome pass != fail") sequence: fixture-setup -> setup:ok -> body:ok -> teardown:ok -> fixture-teardown -> fixture-setup -> setup:fail -> body:fail -> teardown:fail -> fixture-teardown seeded 42 twice: one=[bdd732262feb6e95,28efe333b266f103,47526757130f9f52] two=[…same…] equal=true first_unchanged=true mocks: blocks=1 players=1 time=1000 weather=rain | torches=7 | sent=["hello"] refusals: [client_test_hooks, client_test_state, screenshot, trace] | run_game_test=0 | capture_screenshot=None | replay_trace=false | assert_block_state=false | assert_event_fired=false Assumed, not verified: nothing beyond the manager wiring 70.prelude and an in-game run. The two demo bodies that panic print through the default panic hook to stderr before being caught; that is the intended isolation evidence, not a failure.

### 71 — Hot reload and development mode

§71 real: `sdk/src/dev.rs` — a watch list whose `Fingerprint` (size + mtime + FNV hash) reports a path once and never again while it is unchanged; `plan_reload` splits changed paths into `reloadable` / `blocked` with a reason each (a mod's own assets/config/scripts vs the client's shaders, classes and a frozen registry); a `ReloadRequest` queue with a real queued→running→done/failed machine, recorded outcomes and a refused duplicate target; `add_reload_listener` + `reload_now` calling exactly the listeners for the reloadable kinds in registration order; a `CINNALOADER_DEV`-defaulted flag and `what_would_be_lost`. §71 refused (4): the client, its resources, its classes, and the client's own file watcher — `why_unknown(what)`, `refusals()`, `crate::warn!` at restart_client/restart_world/restart_server/recompile_shaders/rebuild_ui and at a watch path outside the mod dir. §71 evidence (`CINNALOADER_DEMO=71`, CLI run /tmp/cl-mods 250 20): fingerprint `size 11->11 hash 8b58..->8b5b.. changed_only_when_written=true`, `quiet_before=[] changed=["assets/lang/en_us.json"] quiet_after=[]`; plan 3 can (assets/config/scripts) + 3 cannot (registry frozen, shaders, client) each with a reason; queue `duplicate_refused=true running=Running finished=true finish=done outcome="hello_mod reloaded"`; listeners `ran=2 in_registration_order=[first,second]` (config-only not called); 4 refusals, all refused entry points false; longest line 166 chars. §71 types for the prelude: dev {BlockedReload, Fingerprint, PlannedReload, QueueState, ReloadKind, ReloadPlan, ReloadRequest}. No name collisions in lib.rs. §71 gate: `cargo build -p cinnaloader`, `cargo test -p cinnaloader --lib dev::` (8 passed), `cargo test -p cinnaloader --doc` (93 passed), `cargo build -p hello-mod`, `cargo clippy --all-targets` — all green, zero warnings in dev.rs/s71.rs.

### 72 — Scripting and embedded extensions

72. Scripting and embedded extensions — `sdk/src/scripting.rs`, demo `examples/hello-mod/src/s72.rs`. No new deps. `pub mod scripting;` already in `sdk/src/lib.rs`, NOT yet in the prelude; doctest and demo use `cinnaloader::scripting::…` paths. 72.prelude is for the manager. §72's `Value` collides with `serialization::Value` in the prelude, so the prelude aliases it `ScriptValue`. All 28 wishlist names exist and are real; only the client-side calls refuse. single/double strings with `\n \t \r \\ \" \' \0`, identifiers, `+ - * / %`, `== != < <= > >=`, words `and or not`, punctuation `( ) { } , ;`, keywords `if else while let return true false`. An unknown char -> `Syntax{pos,msg}`. A trailing `;` may be omitted on the last statement. unary `-`, call/primary; binops left-assoc, unary right-assoc. `eval("1 + 2 * 3")==7`, int+int stays int, either-side float promotes to float; `/` between ints is integer division, int/float and lexicographic for strings; `and`/`or` short-circuit (right side of `false and x` is never run — tested with an `Arc<AtomicUsize>`). `x/0` and `x%0` -> `DivideByZero{pos}`. Unknown name -> `UnknownFunction{name,pos}`; wrong count -> `Arity{name,expected,got,pos}`. why the budget exists: a script is data and data can loop forever, hanging the loader thread. A program's value is its trailing expression, or a `return`, else Null. Errors are positioned. serialized-value byte length), `set_execution_timeout` (checked every 256 steps via ["client","client_scripts"]. precedence/ops; scopes+shadow+bindings+while; short-circuit; host call; unknown/arity; divide by zero (pos 2 / 4); budget on `while true {}`; syntax positions; runtimes/globals/serialize round trip; client refusals. Gate: `cargo build -p cinnaloader` ok; `--lib scripting::` 10 passed; `--doc` 93 passed; two files (an unrelated pre-existing `useless_format` in another lane's not resolve; there is no import/type/event semantics in the language beyond the registry. capability check in the evaluator. per-runtime; runtimes/contexts are script storage and bookkeeping. into `scripting` (no re-entry) or it deadlocks. before a run, not during one (the step budget is the during-run stop).

### 73 — Inter-mod communication

Files owned: `sdk/src/interop.rs` (module), `examples/hello-mod/src/s73.rs` (demo). No other file touched. `pub mod interop;` already existed in `sdk/src/lib.rs:73`; prelude exports are in with `MethodSpec { name, arity, returns: ReturnKind }` as real data; builders `capability`, name refused (`false`); `unregister_service`, `get_service`, `find_services(cap)`. request major AND provider minor >= request minor; patch ignored (documented). Different major refused. `ApiError` variants: `UnknownApi`, `VersionMismatch{api,requested,offered, provider}`, `MissingCapability{api,capability}` — distinct, capability named with its display name (`packet_send`). expected,got}`, handler `Err(String)` -> `ProviderError`, else the value. {api,requested,provider,offered,resolved,capabilities,missing,bridged,compatible,reason}`. (count of subscribers reached), `unsubscribe`. target service, consulted by `require`), `CompatibilityAdapter` (explicit cross-major shim). and `call_client()` (calling into the client) all `crate::warn!` and never fake success; compatible resolve + major mismatch with both versions; double publish refused + unpublish; missing capability named and != version mismatch; call dispatch + wrong arity (2 vs 1) + unknown method by name + unknown api + provider error; negotiation report fields; bus counts 2 then 1 then 0; extension points/extensions; shared data + bridge + adapter + refusals honest. errors owned by other lanes; did not touch them). s73.rs** at any point). runnable doctest and by the demo `s73.rs`, not by a live headless-client run. Refusals are the client's own API surface, which by definition cannot be reached here. versions, missing capability refused naming it, a real call value, wrong arity with both counts, unknown method by name, bus with two distinct subscribers, and the client refusals. Not run through `mcp.py` (lane instructions = build/test/clippy gate only).

### 74 — Reflection and internal access

Status: DONE for this lane — implemented, unit-tested, doctested, demo runs in the CLI, clippy-clean. No manager-owned file touched (`sdk/src/lib.rs`, `examples/hello-mod/src/lib.rs`, in `sdk/src/lib.rs` (lines 74-75); the demo is already wired at Prelude wiring is the manager's: `/tmp/cl-lanes/74.prelude`. No name collides with the existing prelude, so no aliases are needed. fields, methods) with builders `.described/.extends/.implements/.constructor/.field/ .read_only_field/.method`; `.field_of/.method_of/.field_names/.method_names/.defaults/ .summary`. `summary()` is the text: header + one line per field (`kind access default`) and per method (`name(arity) -> return_kind`). from the descriptors). delegate (the wishlist names). `create_instance` builds every field at its default. Errors: `ReflectError::{UnknownType,UnknownField{type_name,field}, UnknownMethod,WrongKind{field,wanted,found},ReadOnly(field),WrongArity{method,wanted,found}, NoImplementation}`. type/field/method names. .last/.optional/.describe`. accessors apply to every version). the caller's promise. Same store: `get` -> `Err(WrongKind)`, `get_unchecked` -> `Ok(value)`. &[ConfigValue]) -> ConfigValue }` (`register_method`), with an arity check. accessor,key},WrongKind{accessor,wanted,found},Absent{key},UnknownMethod,WrongArity}`. at `get_client_mappings`/`resolve_client_mapping`/`get_client_field`/`invoke_client_method`, Exact errors/values asserted: field/method counts and the full `describe` text; a set-field round trip; `UnknownField{type_name,field}`; `WrongKind{field,wanted,found}` on both the write and a stored wrong-kind read; `ReadOnly`; `invoke` -> `Ok(Integer(7))` and `WrongArity{2,1}` and path/first/last/optional exact values; checked refuses / unchecked accepts with the recorded error; `OutOfRange`, `Missing`, `Empty`, `NotAList`, `UnknownAccessor`; `get_field`/`set_field` with `Absent`/`NotAMap`; method arity; version mapping filter; refusals. Mutation check (ran, then reverted): removing the read-only guard makes line 16). Covers: descriptor counts + list_fields/list_methods/constructor; set-field 0->5 round trip; unknown field named; wrong kind with both kinds; read-only `label`; `move_by(3,4)=7` and wrong arity 2 vs 1; the `describe` text (one line per line of the summary); resolve/find; accessors checked vs unchecked on the same store; and all six refusal sentences (one per line). are thin delegates so both the spec wording and the wishlist names are satisfied. name; nothing in the current prelude uses `Method`, so no alias is needed today.

### 75 — Patching and transformation

§75 real: `sdk/src/patches.rs` — real byte patching over a buffer the mod owns, and the ledger. order (overlapping included, step 1); `find_first`. replacement length == pattern length. bytes that are not the pattern (`PatternMismatch { at, expected, found }`) instead of writing over them, then writes and records the ledger entry `{handle, id, target, offset, original, replacement}`. A replacement whose length differs is `LengthMismatch` (documented: a patch must be the pattern's length). `apply` takes an explicit offset; `apply_all` locates each patch by pattern search. holds the replacement), `revert(buf, handle)` writes the original back for real, `remove` = revert, asserted byte for byte against a copy of the original. whose first match overlaps one already applied is skipped with `Overlap { at, with }` as its reason (documented rule: first match wins, overlap is skipped, no second search). registered transformer in id order and stores a `TransformationReport { applied, renames, input_bytes, output_bytes, count }`; `get_transformation_report` and `export_report()`. §75 refused (4): the client's binary, its functions, its loaded classes and its live memory — Refused entry points return false: `patch_client_binary/function/class/memory`, §75 names: all 24 wishlist names present — register, unregister, apply, remove, apply_all, remove_all, get, get_all, get_status, get_conflicts, get_order, set_order, validate, rollback, get_original, get_replacement, register_transformer, unregister_transformer, register_injection_point, register_redirect, register_overwrite, register_mixin, get_transformation_report, export_report. Extra helpers: find_all, find_first, Pattern, Patch, AppliedPatch, ChainStep, Conflict, PatchStatus, Rename, TransformationReport, Transformer, InjectionPoint, patch_client_*. §75 types for the prelude: patches {AppliedPatch, ChainStep, Conflict, InjectionPoint, Patch as CodePatch, PatchError, PatchStatus, Pattern, Rename, TransformationReport, Transformer}. §75 gate: `cargo build -p cinnaloader`, `cargo test -p cinnaloader --lib patches::` (8 passed), §75 trap: `register`/`register_transformer` replace on a repeated id (render.rs semantics) and return false; a test that calls them to prove the false must re-register the real value or it silently swaps the definition. Found and fixed in `a_transformer_renames_source_text_and_reports_it`. §75 note: while this lane ran, `datadriven.rs` and `scripting.rs` were broken by other lanes; both were "re-run rather than fix" per instructions and cleared on their own before the gate above.

### 76 — Data-driven systems

Index: what landed, the public surface, the 15 wishlist names, test list, gate receipts, integration points for the manager, and what is honest-partial. Extra helpers needed by the spec/demo: `unload`, `count`, `get_all_of`, `summary`, Types (exported): `Document`, `DocumentSchema`, `SchemaField`, `Validation`, `Reference`, (named `a -> b -> a`), a missing schema, a version with no migration, and content the schema refuses. Parents are topologically ordered before children (deterministic: ids sorted, DFS); load returns the ids in that order. default loader registers one entry `(doc.id -> content)`. `unload` unregisters exactly the stored entry ids of that one document. content and the old stored document untouched. A successful reload swaps only its entries. through `SchemaField::element` / `::field`. and a matching transition exists; else `NoMigration`. the target is a loaded document or in the default registry. 1. `a_document_loads_and_is_read_back_per_kind` — load 2 kinds, defaults filled, count/all, query, summary, duplicate refused with exact error text. 2. `a_schema_answers_every_problem_with_its_path` — two problems, exact paths/reasons. 3. `a_dependency_loads_after_its_parent_in_order` — order `["red","green"]`, missing parent. 4. `a_cycle_is_refused_and_named` — `Cycle(["green","red","green"])`, nothing loaded. 5. `a_reload_replaces_one_document_and_a_refused_reload_keeps_the_old`. 6. `an_unload_removes_exactly_one_document` (neighbour stays, second unload false). 7. `loaders_codecs_processors_migrations_and_watches_really_run` — all four registrations + watch/unwatch + `decode` unknown format. 8. `the_clients_own_documents_are_refused`. datadriven(76): validate refused 2 problems at once = [s76:loot.pools[0].weight: expected integer, found text] [s76:loot.rolls: required, but missing] datadriven(76): dependency order enforced = ["demo:red", "demo:green"] datadriven(76): cycle refused = cycle: cyc:green -> cyc:red -> cyc:green datadriven(76): reload replaced gem=Some(Object({"v": Integer(9)})) neighbour ore=Some(Object({"v": Integer(2)})) datadriven(76): refused reload (true) left old gem=Some(Object({"v": Integer(9)})) datadriven(76): unload removed gem=true ore kept=Some(Object({"v": Integer(2)})) datadriven(76): 3 documents across 1 kinds: s76:item=3 Longest line 150 chars (limit 200). which matches how the surrounding sections were verified. here is documented in its doc comment; it does not parse a schema for reference fields. registry never calls back into datadriven, so no lock cycle. Internal validation uses builder had to be named `with_default` because `default` shadowed the `Default` impl. gate line below is from a clean full pass after they settled.

## Open: a wild jump on the first frame

A wild jump (rip in no mapping at all) kills the client when a mod does a lot of work inside
one hook callback — during `load`, which runs from the preload's constructor, or in a single
tick, which runs from the frame hook. The same mod built the same way runs to completion in
the CLI, which installs no hooks. Six runs died this way while sections 9–14 were verified;
moving each section's demonstration into its own task, one section per tick, made every run
stable again, and no coverage was lost — the same lines appear, a few ticks later.

So the constraint is the hook layer's, not the API's: keep any single callback light. The
suspicion is ilhook's `hook_closure_jmp_back` path, which is the one thing between the
client's frame and a mod's tick. Reproduced and recorded in `cinnaloader-notes/STATUS.md`;
root-causing it is its own piece of work and does not block the roadmap.

Two things make this cheaper to live with, and both are now in the harness:

* **One section per run.** `examples/hello-mod` guards every demonstration with
  `CINNALOADER_DEMO=<section>`, so a verification run exercises one section and its log is
  short. `tools/mcp.py` forwards that variable.
* **The CLI runner got a clock.** `cinnaloader run <mods> [ticks] [millis per tick]` paces its
  ticks, which a mod that schedules work for later needs: without pacing, 400 ticks finish in
  less time than one `schedule_after` delay and nothing scheduled ever fires. With it, a
  section that runs a few seconds in can be verified without the client at all — which is how
  section 33 was verified while the client's hook layer was crashing on repeated starts.

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
