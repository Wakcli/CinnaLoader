# Seeing the loader work in-game

The only accepted proof that an API works is a run of the real client with the loader
attached. This is the recipe that works on this machine; keep it current when it changes.

Every path below is absolute on purpose.

## 0. Before anything

```sh
pkill -f 'target/debug/cinnabar --address'   # leftovers fight over the GPU
```

A leftover client shows up as a `bevy_render::error_handler: Caught DeviceLost` exit or a
login failure, which looks like a bug in the loader and is not.

## 1. The test world (leave it running)

```sh
cd /home/wak/Cinna/cinnabar
tail -f /dev/null | ./target/debug/bedrock-local-server \
  -dir .local/cinnaloader-testworld -addr 127.0.0.1:19132 \
  -name "Cinnaloader Test Flat" -generator flat -game-mode creative -difficulty peaceful
```

`tail -f /dev/null |` keeps stdin open; without it the server exits on EOF. If the client
logs `disconnectionScreen.cantConnect`, this server is not running — restart it, that is
not a loader problem.

## 2. A client that can host the loader

The in-game menu is the client's own panel, and that API only exists in a `local-mods`
build. `developer-control` is what the headless driver talks to.

```sh
cd /home/wak/Cinna/cinnabar
make play-build PROFILE=dev CLIENT_FEATURES="--features local-mods,developer-control"   # ~9 min
cargo build -p cinnabar-mcp                                                             # the driver's server
```

## 3. Build the loader and stage a mod

```sh
cd /home/wak/Cinna/cinnaloader/cinnaloader
cargo build && cargo test && cargo clippy --all-targets
mkdir -p /tmp/cl-mods && cp target/debug/libhello_mod.so /tmp/cl-mods/
```

`/tmp` is wiped between boots on this box. If the mods directory is missing the preload
exits early and nothing happens at all, silently.

Quick check without the client (loads, ticks and unloads the mod):

```sh
target/debug/cinnaloader run /tmp/cl-mods
```

That path also writes each mod's `<mods_dir>/<mod>.cfg`, which is the fastest way to see
registration, ticking and persistence without a client.

## 4. Run the client headless and look at it

```sh
python3 /home/wak/Cinna/cinnaloader/tools/mcp.py \
  launch sleep:45 shot:/tmp/opencode/panel.png
```

`tools/mcp.py` wraps Cinnabar's client MCP (stdio JSON-RPC). It launches the client hidden
(`headless: true`), with `LD_PRELOAD` pointing at the loader and `CINNALOADER_DIR` at the
mods directory, and `CINNALOADER_MENU_OPEN=1` so the menu is open without a key press.
Commands: `launch`, `sleep:SECONDS`, `shot:PATH`, `click:X,Y`, `quit`.

Read the client's log for the run:

```sh
L=/home/wak/Cinna/cinnabar/.local/developer-control/logs/$(ls -t \
  /home/wak/Cinna/cinnabar/.local/developer-control/logs/ | head -1)
grep -a cinnaloader "$L" | grep -v 'alive:'
```

Then **look at the screenshot**. A capture that shows the effect is the evidence; a log
line on its own is not.

## 5. What this recipe cannot do

- **Keybinds and the Shift key.** `device_query` reads the X keyboard and only sees keys
  while the game window has focus. A hidden window never has focus, so key-driven APIs are
  verified in the CLI or by a unit test and marked as not verified in-game.
- **Mouse clicks on the menu.** `tools/mcp.py click:` moves the pointer, but synthetic
  input does not reach the panel's own pointer path, and clicks are not wired to settings
  anyway.
- **Anything that needs a visible window.** Do not open one for automated captures.

## 6. When it crashes

```sh
coredumpctl list --no-pager | tail -3
coredumpctl dump <pid> -o /tmp/opencode/core
gdb -batch -ex 'bt 12' -ex 'info registers rip rsp rdi' \
  /home/wak/Cinna/cinnabar/target/debug/cinnabar /tmp/opencode/core
```

Compute the offset from the load base — the base is the address the loader logged for its
hook minus the hook's link-time offset (`syms.find("capture_hud_frame")` via
`/tmp/opencode/symprobe`, see the notes). An `rip` in the executable's range but outside
its mapped image means a wild jump, which so far has always been a hook, never the panel.
