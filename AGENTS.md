# cinnaloader — project instructions for an agent

This folder is the project. The loader itself is the Rust workspace in `cinnaloader/`.

```
cinnaloader/            the Rust workspace: sdk, loader, native, preload, launcher, examples
apis.md                 the API list and its status — this is the work queue
docs/testing-in-game.md the verified recipe for launching and seeing the loader in-game
tools/mcp.py            headless client driver used by that recipe
```

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

## Cinnabar reference

The client this loader attaches to is Cinnabar. Its own instructions for agents, including
how to launch and capture it, are authoritative and local:

- `/home/wak/Cinna/cinnabar/docs/agents/multi-agent-workflow.md` — worktrees and build limits
- `/home/wak/Cinna/cinnabar/docs/agents/live-testing.md` — rendered-frame evidence
- `/home/wak/Cinna/cinnabar/docs/agents/client-mcp.md` — scripted headless sessions

Upstream: <https://github.com/bedrock-mc/cinnabar/tree/dev/docs/agents>. Read the local
copies, not a remembered version of them.
