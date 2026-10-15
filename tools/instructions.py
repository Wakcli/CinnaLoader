#!/usr/bin/env python3
"""Read the user's `new-instructions.md` and say which instructions are new.

The user edits `new-instructions.md` on GitHub. This reads the newest copy of it from
`origin/main` or `origin/dev`, works out which `##` headings have not been seen before (by a
short hash of the text, not by number, so reordering or renumbering does not confuse it),
and records what has been dealt with in `instruction-log.md`.

    python3 tools/instructions.py check        # fetch, then list instructions not yet logged
    python3 tools/instructions.py list         # what has been logged so far
    python3 tools/instructions.py log <id> --note "what was done"
    python3 tools/instructions.py fetch        # fetch only, and say what each ref holds

`check` prints `NEW <id> :: <first line>` for each unseen instruction, or `none` when there
is nothing new. Exit code is 0 either way: an empty answer is not a failure.

Nothing here ever writes to `new-instructions.md`.
"""

import argparse
import hashlib
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# The file, the log and the refs are overridable so the reader can be pointed at a fixture
# without touching the user's file.
FILE = os.environ.get("INSTRUCTIONS_FILE", "new-instructions.md")
LOG = Path(os.environ.get("INSTRUCTIONS_LOG", ROOT / "instruction-log.md"))
REFS = os.environ.get("INSTRUCTIONS_REFS", "origin/main,origin/dev").split(",")
INCLUDE_LOCAL = os.environ.get("INSTRUCTIONS_INCLUDE_LOCAL", "1") != "0"
MARKER = "## Instructions"


def git(*args, check=True):
    result = subprocess.run(
        ["git", "-C", str(ROOT), *args],
        capture_output=True,
        text=True,
    )
    if check and result.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def fetch():
    git("fetch", "origin", "--quiet")


def remote_text(ref):
    """The file as that ref has it, or None when the ref has no such file."""
    result = subprocess.run(
        ["git", "-C", str(ROOT), "show", f"{ref}:{FILE}"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return None
    return result.stdout


def candidates():
    """Every remote copy, plus the working tree, so a local edit is honoured too."""
    found = []
    for ref in REFS:
        text = remote_text(ref)
        if text is not None:
            found.append((ref, text))
    local = ROOT / FILE
    if INCLUDE_LOCAL and local.exists():
        found.append(("working tree", local.read_text()))
    return found


def instructions(text):
    """(id, first line, body) for each `##` heading under the marker."""
    start = text.find(MARKER)
    body = text[start + len(MARKER) :] if start >= 0 else text
    found = []
    for block in re.split(r"^## +", body, flags=re.M)[1:]:
        lines = block.strip().split("\n")
        title = lines[0].strip()
        if not title:
            continue
        whole = block.strip()
        digest = hashlib.sha1(re.sub(r"\s+", " ", whole).encode()).hexdigest()[:12]
        found.append((digest, title, whole))
    return found


def logged():
    if not LOG.exists():
        return {}
    seen = {}
    for line in LOG.read_text().split("\n"):
        match = re.match(r"^\|\s*`([0-9a-f]{12})`\s*\|([^|]*)\|([^|]*)\|(.*)\|\s*$", line)
        if match:
            seen[match.group(1)] = match.group(4).strip()
    return seen


def newest():
    found = candidates()
    if not found:
        return None, None
    # The newest copy wins, and anything only another copy has is still reported, so an
    # instruction written on one branch is never missed.
    best = found[-1]
    merged = {}
    for _, text in found:
        for digest, title, whole in instructions(text):
            merged.setdefault(digest, (title, whole))
    return best[0], merged


def check():
    fetch()
    where, found = newest()
    if found is None:
        print("none: no new-instructions.md on origin/main or origin/dev yet")
        return
    seen = logged()
    fresh = [(digest, title) for digest, (title, _) in found.items() if digest not in seen]
    if not fresh:
        print(f"none: {len(found)} instruction(s), all logged already (newest copy: {where})")
        return
    for digest, title in fresh:
        print(f"NEW {digest} :: {title}")
    print(f"({len(fresh)} new of {len(found)} in the newest copy: {where})")


def list_log():
    seen = logged()
    if not seen:
        print("nothing logged yet")
        return
    for digest, note in seen.items():
        print(f"{digest} {note}")


def write_log(digest, note):
    fetch()
    _, found = newest()
    title = ""
    if found and digest in found:
        title = found[digest][0]
    if not LOG.exists():
        LOG.write_text(
            "# Instructions read and dealt with\n\n"
            "One line per instruction from `new-instructions.md`, keyed by the first twelve\n"
            "characters of a hash of its text, so an instruction is never processed twice and\n"
            "the user's file is never written to.\n\n"
            "| id | first seen | what it was | what was done |\n"
            "| --- | --- | --- | --- |\n"
        )
    stamp = git("log", "-1", "--format=%cI").strip()
    with LOG.open("a") as log:
        log.write(f"| `{digest}` | {stamp} | {title} | {note} |\n")
    print(f"logged {digest}: {title or '(not in the newest copy)'} — {note}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["check", "list", "log", "fetch"])
    parser.add_argument("id", nargs="?")
    parser.add_argument("--note", default="done")
    args = parser.parse_args()

    if args.command == "check":
        check()
    elif args.command == "list":
        list_log()
    elif args.command == "fetch":
        fetch()
        for ref in REFS:
            text = remote_text(ref)
            print(f"{ref}: {'present' if text is not None else 'absent'}")
    elif args.command == "log":
        if not args.id:
            sys.exit("log needs an id, as printed by check")
        write_log(args.id, args.note)


if __name__ == "__main__":
    main()
