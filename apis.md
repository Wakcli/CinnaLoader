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
