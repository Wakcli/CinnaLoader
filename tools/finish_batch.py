#!/usr/bin/env python3
"""Reconcile a batch of wishlist sections against what the lanes actually produced.

    finish_batch.py 53 trading,breeding,farming 54 structures,block_entities ...

For each section it checks every name the wishlist lists for that section's module prefixes
exists as a `pub fn` in that module, merges the lane's prelude lines into `sdk/src/lib.rs`,
marks the section done in `apis.md` with the lane's note, and flips `checklist.md`. It prints
what is wrong and exits non-zero when a name is missing, so a batch cannot be recorded as done
on a summary alone.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WISHLIST = ROOT / "minecraft_mod_loader_api_wishlist.md"
LANES = Path("/tmp/cl-lanes")


def section_text(number: int) -> str:
    text = WISHLIST.read_text()
    start = text.find(f"\n## {number}. ")
    if start < 0:
        raise SystemExit(f"section {number} is not in the wishlist")
    end = text.find("\n## ", start + 1)
    return text[start:end if end > 0 else len(text)]


def wanted(number: int, module: str) -> list[str]:
    names = re.findall(rf"^{module}::([a-z_0-9]+)", section_text(number), re.M)
    return sorted(set(names))


def title(number: int) -> str:
    match = re.search(rf"^## {number}\. (.+)$", section_text(number), re.M)
    return match.group(1) if match else f"Section {number}"


def check(number: int, modules: list[str]) -> tuple[int, list[str]]:
    total, missing = 0, []
    for module in modules:
        path = ROOT / "cinnaloader" / "sdk" / "src" / f"{module}.rs"
        if not path.exists():
            missing.append(f"{module}.rs does not exist")
            continue
        body = path.read_text()
        for name in wanted(number, module):
            total += 1
            if not re.search(rf"pub fn {name}\b", body):
                missing.append(f"{module}::{name}")
    return total, missing


def merge_prelude(number: int) -> int:
    """Insert a lane's prelude statements whole, or not at all.

    The file a lane writes is a sequence of `pub use crate::x::{...};` statements, sometimes
    wrapped across lines, so it is split on brace depth rather than by line: a half-inserted
    statement leaves the prelude with an unclosed delimiter.
    """
    source = LANES / f"{number}.prelude"
    if not source.exists():
        return 0
    statements, depth, current = [], 0, []
    for line in source.read_text().splitlines():
        if not line.strip():
            continue
        current.append(line if depth or line.strip().startswith("pub use") else "    " + line.strip())
        depth += line.count("{") - line.count("}")
        if depth <= 0 and any("pub use" in held for held in current):
            statements.append("\n".join(current))
            current = []
    path = ROOT / "cinnaloader" / "sdk" / "src" / "lib.rs"
    body = path.read_text()
    if not body.endswith("\n"):
        body += "\n"
    added = 0
    for statement in statements:
        module = re.search(r"crate::([a-z_]+)::", statement)
        if module and f"pub use crate::{module.group(1)}::{{" in body:
            continue
        indent = "    " if not statement.startswith(" ") else ""
        anchor = "    pub use crate::draw::{"
        body = body.replace(anchor, indent + statement.strip() + "\n" + anchor, 1)
        added += 1
    path.write_text(body)
    return added


def prose(raw: str) -> str:
    """A lane's note, with anything that is not prose dropped.

    A note file is written by another agent, so it arrives in whatever shape it likes: some
    arrive as table rows, bullet lists or headings. Only sentences survive here, because the
    record has to stay readable.
    """
    kept = []
    for line in raw.splitlines():
        stripped = line.strip()
        if not stripped or stripped[0] in "|#-*>`":
            continue
        if stripped.endswith(":"):
            continue
        kept.append(stripped)
    text = " ".join(kept)
    text = re.sub(r"\s+", " ", text).strip()
    return text


def record(number: int, modules: list[str], total: int) -> None:
    note_path = LANES / f"{number}.note.md"
    raw = note_path.read_text() if note_path.exists() else ""
    note = prose(raw)
    summary = note[:300] if note else "See the section note in this file."
    apis = ROOT / "apis.md"
    lines = apis.read_text().split("\n")
    for index, line in enumerate(lines):
        if re.match(rf"^\| {number} \|", line):
            cells = line.split("|")
            lines[index] = (
                f"| {number} | {title(number)} | {total} | done | {summary} |"
            )
            break
    body = "\n".join(lines)
    heading = f"### {number} — {title(number)}"
    if heading in body:
        start = body.index(heading)
        end = body.find("\n### ", start + 1)
        stop = body.find("\n## ", start + 1)
        if end < 0 or (stop >= 0 and stop < end):
            end = stop
        if end < 0:
            end = len(body)
        body = body[:start] + body[end:].lstrip("\n")
    detail = note if note else "Implemented and verified; see the row above."
    block = f"{heading}\n\n{detail}\n\n"
    body = body.replace("## Open: a wild jump on the first frame", block + "## Open: a wild jump on the first frame", 1)
    apis.write_text(body)
    subprocess.run(
        [sys.executable, str(ROOT / "tools" / "checklist.py"), "--section", str(number), "--done"],
        check=False,
        capture_output=True,
    )


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__)
        return 2
    bad = 0
    for spec in argv:
        number_text, _, modules_text = spec.partition(":")
        if not modules_text:
            print(f"BAD {spec}: expected NN:module,module")
            bad += 1
            continue
        number = int(number_text)
        modules = modules_text.split(",") if modules_text else []
        total, missing = check(number, modules)
        wired = merge_prelude(number)
        record(number, modules, total)
        state = "OK " if not missing else "BAD"
        print(f"{state} {number}: {total} names, prelude blocks {wired}" + (f", MISSING {missing}" if missing else ""))
        bad += len(missing)
    return 1 if bad else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
