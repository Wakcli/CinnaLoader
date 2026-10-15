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

INST-002: Smart Parallel Agent Usage and Coordination

Make effective use of multiple AI agents to accelerate CinnaLoader development, including writing code, researching Minecraft internals, testing, reviewing, and documentation.

- Use parallel agents when tasks can genuinely be completed independently, including implementing separate API sections or self-contained components.
- Keep one main agent responsible for architecture, coordination, integration, final verification, commits, and pushes.
- Give every agent clear, specific instructions, relevant context, defined responsibilities, and explicit completion criteria.
- Create and maintain good agent instruction files, including "AGENTS.md" and any relevant task-specific instructions. Ensure agents read and follow them rather than treating them as optional documentation.
- Use agents intelligently and economically. They consume API budget, so do not spawn agents for trivial tasks, simple file edits, work the main agent can handle more efficiently, or unnecessary repeated reviews. Choose the number of agents based on task complexity and expected benefit.
- Avoid multiple agents editing the same files simultaneously. Use separate branches or worktrees when appropriate, and coordinate dependencies before assigning work.
- Require implementation agents to build and test their changes and document what they actually implemented. The main agent must review and integrate their work, resolve conflicts, run the required project-wide checks, and verify the final result before committing and pushing.
- Prioritize correctness, security, maintainability, and genuine progress over maximizing agent count or raw output.
- Continue the existing API roadmap and process all instructions through the established "new-instructions.md" workflow. Do not duplicate completed instructions or interrupt ongoing work unnecessarily.
