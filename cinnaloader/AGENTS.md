# cinnaloader — workspace instructions

The project's goal and working agreement live one level up, in `../AGENTS.md`. Read that
file: it holds the standing goal (`minecraft_mod_loader_api_wishlist.md` as the roadmap for
the whole project), the rule that every API is verified in-game before it is called done,
and the facts that keep the loader honest (ABI bumps, where hooks may be installed, the
`local-mods` build requirement).

This directory is just the Rust workspace. Session notes are at
`/home/wak/Cinna/cinnaloader-notes/STATUS.md`.

```sh
cargo build && cargo test && cargo clippy --all-targets
```
