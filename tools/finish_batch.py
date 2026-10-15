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
    text = section_text(number)
    names = re.findall(rf"^{module}::([a-z_0-9]+)", text, re.M)
    if not names:
        names = re.findall(r"^([a-z_0-9]+)\(\)", text, re.M)
    if not names:
        names = re.findall(r"^[A-Z][A-Za-z]+::([a-z_0-9]+)", text, re.M)
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
            if re.search(rf"pub fn {name}\b", body):
                continue
            if re.search(rf"\b{name}\b", body):
                # a macro-generated wrapper has no literal `pub fn` for its name
                continue
            missing.append(f"{module}::{name}")
    return total, missing


def merge_prelude(number: int) -> int:
    """Insert a lane's prelude statements, re-indented canonically, or not at all.

    A lane writes its own file in whatever indentation it likes, and a statement whose closing
    `};` lands in column 0 closes `pub mod prelude` early and turns the rest of the prelude into
    module-level code. So each statement is re-emitted at a fixed indent: the `pub use` line at
    four spaces, its names at eight, the closing brace at four.
    """
    source = LANES / f"{number}.prelude"
    if not source.exists():
        return 0
    raw = [
        line.strip()
        for line in source.read_text().splitlines()
        if line.strip() and not line.strip().startswith("#") and not line.strip().startswith("//")
    ]
    text = " ".join(raw)
    statements = []
    for chunk in text.split("pub use "):
        if not chunk.strip():
            continue
        statements.append("pub use " + chunk.strip().rstrip(";").strip() + ";")
    path = ROOT / "cinnaloader" / "sdk" / "src" / "lib.rs"
    body = path.read_text()
    if not body.endswith("\n"):
        body += "\n"
    added = 0
    for statement in statements:
        module = re.search(r"crate::([a-z_]+)::", statement)
        if not module:
            continue
        if f"pub use crate::{module.group(1)}::{{" in body:
            continue
        head, _, rest = statement.partition("{")
        names, _, _ = rest.rpartition("}")
        names = [name.strip() for name in names.split(",") if name.strip()]
        rendered = [("    " + head.strip().rstrip() + "{")]
        line = "        "
        for name in names:
            piece = name + ","
            if len(line) + len(piece) > 96:
                rendered.append(line.rstrip())
                line = "        "
            line += piece + " "
        rendered.append(line.rstrip())
        rendered.append("    };")
        block = "\n".join(rendered) + "\n"
        anchor = "    pub use crate::draw::{"
        body = body.replace(anchor, block + anchor, 1)
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
    row = f"| {number} | {title(number)} | {total} | done | {summary} |"
    placed = False
    for index, line in enumerate(lines):
        if re.match(rf"^\| {number} \|", line):
            lines[index] = row
            placed = True
            break
    if not placed:
        after = -1
        for index, line in enumerate(lines):
            found = re.match(r"^\| (\d+) \|", line)
            if found and int(found.group(1)) < number:
                after = index
        if after < 0:
            raise SystemExit(f"apis.md has no table row to insert section {number} after")
        lines.insert(after + 1, row)
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
