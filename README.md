# cinnaloader

A native mod loader for Cinnabar: mods are Rust cdylibs that the loader loads into the
client and drives from a tick thread, with the in-game menu drawn by the client's own UI.

| | |
| --- | --- |
| `AGENTS.md` | how to work in this project, including the rule that every API is verified in-game |
| `apis.md` | the API list and its status — the work queue |
| `docs/testing-in-game.md` | the recipe for launching the client and seeing the loader work |
| `tools/mcp.py` | headless client driver used by that recipe |
| `cinnaloader/` | the Rust workspace; its `README.md` documents the mod API |

Session notes, findings and open items live outside the repo:
`/home/wak/Cinna/cinnaloader-notes/STATUS.md`.

## Quick start

```sh
cd cinnaloader
cargo build && cargo test
target/debug/cinnaloader run /tmp/cl-mods          # load, tick, unload a mod
target/debug/cinnaloader launch <CLIENT> --mods /tmp/cl-mods   # attach to a client
```

Writing a mod, and the full host API, are in `cinnaloader/README.md`.
