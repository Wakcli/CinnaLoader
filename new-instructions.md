# New instructions

This file is yours. Write one instruction per `##` heading below the marker, in any order,
whenever you like — editing it on GitHub is enough, no message needed.

**The agent never deletes, rewords, reorders or truncates what is here.** It only reads.

What the agent does with it:

* after every push, and before starting each new section of
  `minecraft_mod_loader_api_wishlist.md`, it fetches `origin/main` and `origin/dev` and reads
  whichever has the newest version of this file;
* anything it has not seen before becomes a task, is added to `checklist.md`, and is recorded
  in `instruction-log.md` (a short id, when it was seen, and what was done about it) so the
  same instruction is never processed twice;
* work already in progress is never dropped for a new instruction: the new one is queued
  behind it unless it says otherwise.

An instruction is one `##` heading and the lines under it. The first line is what the agent
shows in its progress updates, so keep it short.

## Instructions

## 1. Work the wishlist to the end

`minecraft_mod_loader_api_wishlist.md` is the roadmap. Go through it top to bottom, one
section at a time: implement the section, test it, verify it in a running client, mark it in
`apis.md` and `checklist.md`, commit it to `dev`, push, then start the next one. Never stop
until every name in that file is either implemented or marked with the reason this client
cannot host it.

## 2. Read this file before every new section

Check `new-instructions.md` after every push and before starting each API section. Anything
new goes on the checklist and is followed alongside the roadmap. Never delete an instruction,
and never process the same one twice.
