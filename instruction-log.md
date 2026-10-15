# Instructions read and dealt with

One line per instruction from `new-instructions.md`, keyed by the first twelve
characters of a hash of its text, so an instruction is never processed twice and
the user's file is never written to.

| id | first seen | what it was | what was done |
| --- | --- | --- | --- |
| `6dc30083b6a2` | 2026-10-15T03:05:10Z | 1. Work the wishlist to the end | In progress: sections 1-10 done and verified in-game, working through 11-107 next |
| `c931d6d01498` | 2026-10-15T03:05:10Z | 2. Read this file before every new section | Done: tools/instructions.py reads origin/main and origin/dev, instruction-log.md records what was dealt with, workflow written into AGENTS.md |
| `20009484ae10` | 2026-10-15T03:25:59Z | 1. Work the wishlist to the end | Standing: working the wishlist top to bottom, sections 1-14 done and verified in-game |
| `3641c253c3ae` | 2026-10-15T03:25:59Z | 2. Read this file before every new section | Standing: check runs after every push and before every section; log is instruction-log.md |
| `9ec90c818575` | 2026-10-15T03:25:59Z | INST-002: Smart Parallel Agent Usage and Coordination | Adopted: one research agent maps upcoming sections in the background while the main agent implements; implementation stays with the main agent to keep the ABI and integration coherent |
