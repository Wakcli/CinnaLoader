# cinnaloader — project instructions for an agent

This folder is the project. The loader itself is the Rust workspace in `cinnaloader/`.

```
cinnaloader/            the Rust workspace: sdk, loader, native, preload, launcher, examples
apis.md                 the API list and its status — this is the work queue
checklist.md            one line per API named in the wishlist, ticked as it lands
docs/testing-in-game.md the verified recipe for launching and seeing the loader in-game
tools/mcp.py            headless client driver used by that recipe
tools/checklist.py      flips a whole section's marks: `--section 5 --done`
```

`minecraft_mod_loader_api_wishlist.md` at the project root is **the specification and the
single list to work through** — 107 sections, ~3400 named APIs. Nothing in it is optional
and nothing is dropped silently: every name ends as a working API, or in `apis.md` with a
reason why this client cannot host it. `checklist.md` is the same list with a mark per
name (`[ ]` todo, `[~]` partial, `[x]` done and verified in-game, `[-]` not applicable);
when both agree the work is done.

## Standing instruction: do not stop until the wishlist is done

The instruction from the user, kept verbatim because it is the operating mode:

> FROM HERE ON FORTH YOU ARE NOT TO STOP UNTILL THE MINECRAFT_MOD_LOADER_API_WHISHLIST.MD IS
> DONE NO NEED TO RESPOND TO THIS DIRECTLY JUST FOLLOW THE INSTRUCUTONS

and

> you are now autonomous, you should be running for over 1-3 hours doing this one task […]
> every time you are about to stop speaking (save this to agent file, make sure you have
> confirmed that you have completed the task I told you to do) ALL of it

So:

- Work the next section. Do not pause for confirmation, do not ask which section, do not
  stop because a section ended. The end of a section is the cue for the next one.
- Commit and push to `dev` after **every** section, at minimum — small, frequent pushes
  rather than one large one.
- **Before ending any turn**, update the "Where I am" block below with: the last thing
  finished, the thing in progress and its exact next command, and anything blocked. A turn
  that ends without that update costs the next session its place in the queue.
- A claim of completion is only true when `checklist.md` has no `[ ]` or `[~]` left, and
  every `[-]` has its reason in `apis.md`. Check that before saying the task is done.
- Progress goes to the Discord webhook below at least every ten minutes while working:
  `<https://discord.com/api/webhooks/1560109655575756920/9c8zThZb_FmPeUc4i94GRCSWW0VlvI5j3L5PUNOsg2C_rlVHTihChozaGFaKKhxWuGgQ>`
  (`curl -H 'content-type: application/json' -d '{"content":"..."}' <url>`), one short
  message: the section just finished, the one in progress, the commit hash.

## The user's instructions arrive through `new-instructions.md`

The user writes instructions into `new-instructions.md` in this repository, through GitHub,
without saying anything in chat. That file is theirs: **never delete, reword, reorder or
truncate it.** Read it, act on it, and leave it exactly as it is.

The loop, and it is not optional:

```sh
python3 tools/instructions.py check      # after every push, and before starting each section
python3 tools/instructions.py log <id> --note "what was done"
```

1. `check` fetches `origin/main` and `origin/dev`, reads whichever has the newest copy of the
   file, and prints `NEW <id> :: <first line>` for every instruction that has not been logged.
   An instruction's id is a hash of its text, so editing the wording makes it a new one and
   renumbering does not.
2. Every `NEW` line is a task. Add it to `checklist.md` (and to `apis.md` if it is a section's
   worth of work), then do it — alongside the roadmap, not instead of it.
3. Log it with `instructions.py log` once it is dealt with, saying what was done. The log is
   `instruction-log.md`, and it is what stops the same instruction being done twice.
4. Work in progress is never dropped for a new instruction: finish or park the section first,
   then take the new one.

If `check` prints `none`, there is nothing new and the roadmap continues where it was.

### Where I am

- **Last finished:** §20 entity components and attributes (commit `b71dbc1`). Sections 1–20
  are done and verified in a live client; `apis.md` rows 1–20 say `done` and `checklist.md`
  agrees. A background research agent mapped §15–24 in
  `../cinnaloader-notes/SECTIONS-15-24.md`, so §21–24 can be worked from that mapping.
- **In progress:** starting §21 player APIs.
- **Next command:** `python3 tools/instructions.py check`, then work `## 21. Player APIs`
  from the mapping: the local player's live state is the client's (refuse with a reason);
  what a mod owns — player-shaped values, a hotbar selection it tracks — is real.
- **Pattern the hook-only sections follow, so the next one goes fast:** a real *declaration*
  registry for what a mod ships (`.family`-style builders, read back by id and by name), a
  `why_unknown(what)` sentence per kind of client state, `refusals()` listing them, the
  wishlist's event kinds registered on the §6 bus, and honest `None`/`false`/empty returns
  for everything that would need a hook. `world.rs`, `blocks.rs`, `items.rs`,
  `inventory.rs`, `entities.rs` and `components.rs` are the six to copy from.
- **Blocked (carried forward):** keybinds cannot be exercised headlessly (`device_query`
  needs a focused window) and synthetic clicks do not reach the panel, so those stay
  verified-by-CLI-and-unit-test with the reason recorded in the row.

Session notes live **outside** the repo, at `/home/wak/Cinna/cinnaloader-notes/STATUS.md`.
That split is deliberate: the repo gets the change and its tests, the notes get everything
else (findings, dead ends, open items).

## Goal

Use `minecraft_mod_loader_api_wishlist.md` as the global API roadmap for the project. Your
goal is to build a complete, production-quality Minecraft mod loader that implements as
much of the wishlist as possible, with a strong focus on API coverage, extensibility,
performance, stability, and developer experience.

Treat the document as the long-term target for the entire project, not a task to complete
in one pass. Inspect the existing codebase, identify what is already implemented, and
progressively implement the missing systems in a sensible order. Keep the architecture
clean, APIs consistent, and features genuinely functional rather than creating empty stubs
or placeholder implementations.

Preserve existing working functionality, add tests, update documentation, and keep track of
completed, partial, and missing APIs. Make decisions independently where reasonable,
prioritise foundational systems before dependent features, and continuously work towards
implementing the fullest practical version of the wishlist.

## Working agreement

1. **Work `apis.md` top to bottom, one item at a time.** It is a queue, not a wish list.
2. **For every item: implement, then verify in-game, then move on.** The order is
   `cargo build` → `cargo test` → `cargo clippy` → **in-game verification** → update the
   row in `apis.md`.
3. **In-game verification means a real run of the running client**, using the recipe in
   `docs/testing-in-game.md`: a headless client with the loader attached, the new API
   actually exercised, and evidence captured from *that* run (a screenshot, and the log
   lines it produced). One log line saying it loaded is not evidence that the API works —
   call it, and show the effect.
4. **Never mark a row `done` on the strength of a unit test or a compiling build.** Those
   are necessary and not sufficient. If something cannot be verified in-game, say why in
   the row and leave it `wip` or `blocked`.
5. **Record as you go** in the notes: what changed, the exact commands, what was observed
   versus assumed, and what is still open. Update it again before you stop.
6. **Keep the workspace clean.** The repo gets the change and the tests for it. Scratch
   scripts, probes and captures live in `tools/` here or in `/tmp`, never in the workspace.
7. **Say what you verified and what you did not.** "The client ran for 5 minutes with the
   mod attached and the toggle log appeared" is a verified claim. "This should work" is not
   a claim at all.

## Commits and branches

Commit and push your changes to the current working branch after every significant feature,
major milestone, or worthwhile fix. Keep commits focused and descriptive, ensure tests pass
before pushing where practical, and never push directly to `main` unless explicitly
instructed.

In practice: the working branch is `dev`. `main` is the base it came from; put work on
`dev` and leave `main` alone unless the user asks for it by name. A finished section of
`apis.md` is a commit — say what was implemented, what the in-game evidence was, and which
`apis.md` row it closes.

## Facts that keep the loader honest

- **ABI.** Any change to `HostApi`, the entry points, or a type that crosses the boundary
  bumps `cinnaloader::ABI_VERSION`. The loader refuses a mismatched mod with a message
  rather than loading it, so a stale `.so` in the mods directory shows up as a skip.
- **Hooks.** Only patch before the target's threads exist — the preload constructor. The
  `capture_hud_frame` hook (once per frame, `hook_closure_jmp_back`) is proven. Hooking a
  hot function with `hook_closure_retn` and calling the relocated original crashes the
  client; do not reintroduce that path without solving it first (see the notes).
- **The client's panel API only exists in a `local-mods` build.** Without that feature the
  linker drops `ModUi::set_mod_panel`; the loader then falls back to its text menu.
- **Keys.** The loader reads keys through `device_query`, which is an X11 client, so the
  target is pinned to X11 in the constructor. It only sees keys while the game window has
  focus, which also means headless runs cannot exercise a keybind.
- **Client-side panel clicks are not wired to settings yet.** The menu is driven from the
  loader's own key loop. Do not describe clicks as working.
- **Kill every headless client before the next run.** Leftovers contend for the GPU and
  produce `DeviceLost` exits that look like crashes in our code.
- **A killed client faults in the NVIDIA driver on the way out, loader or not.** A vanilla
  client does it too under `SIGTERM`. `tools/mcp.py` asks the client to quit instead of
  killing it, and a coredump is only worth reading when the run ended on its own.
- **Keep any one mod callback light.** A wild jump — rip in no mapping at all, no
  backtrace — kills the client when a mod does a lot of work inside a single hook callback:
  during `load` (which runs from the preload's constructor) or inside one tick (from the
  frame hook). The same mod runs clean in the CLI, which installs no hooks, so the hook
  layer is the suspect, and ilhook's `hook_closure_jmp_back` is the one thing between the
  client's frame and a mod's tick. Work that has to happen should be split across ticks
  (`tasks::schedule_after_ticks`) rather than done in one go. If a run dies right after
  `menu hooked` or in the first frames, suspect this before the mod.

## Cinnabar reference

The client this loader attaches to is Cinnabar. Its own instructions for agents, including
how to launch and capture it, are authoritative and local:

- `/home/wak/Cinna/cinnabar/docs/agents/multi-agent-workflow.md` — worktrees and build limits
- `/home/wak/Cinna/cinnabar/docs/agents/live-testing.md` — rendered-frame evidence
- `/home/wak/Cinna/cinnabar/docs/agents/client-mcp.md` — scripted headless sessions

Upstream: <https://github.com/bedrock-mc/cinnabar/tree/dev/docs/agents>. Read the local
copies, not a remembered version of them.
