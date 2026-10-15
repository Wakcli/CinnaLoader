#!/usr/bin/env python3
"""Alias exactly the prelude names the compiler says are duplicated.

The crate has eighty-odd modules and each re-exports its own types, so the prelude keeps
colliding: `Node` arrives from `editor` and `server_permissions`, `Change` from `registry` and
`tools`, and so on. Rather than guessing, this reads `cargo build`'s E0252 reports, finds the
module each duplicated line belongs to, and rewrites that binding as `<Module><Name>` — the
module path keeps the wishlist's spelling either way. It loops until the build stops reporting
duplicates, so a batch of new modules costs one call.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LIB = ROOT / "cinnaloader" / "sdk" / "src" / "lib.rs"
DUPLICATE = re.compile(r"the name `(\w+)` is defined multiple times.*?--> (?:[^\s:]+):(\d+):", re.S)


def module_at(lines: list[str], line_number: int) -> str | None:
    for index in range(line_number - 1, max(line_number - 40, -1), -1):
        found = re.match(r"\s*pub use crate::(\w+)::\{", lines[index])
        if found:
            return found.group(1)
    return None


def build() -> str:
    done = subprocess.run(
        ["cargo", "build", "-p", "cinnaloader"],
        cwd=ROOT / "cinnaloader",
        capture_output=True,
        text=True,
    )
    return done.stdout + done.stderr


def main() -> int:
    for _ in range(12):
        output = build()
        found = DUPLICATE.search(output)
        if not found:
            print("no duplicate names left")
            return 0
        name, line_number = found.group(1), int(found.group(2))
        lines = LIB.read_text().split("\n")
        module = module_at(lines, line_number)
        if module is None:
            print(f"cannot tell which module owns the {name} binding on line {line_number}")
            return 1
        alias = f"{module.capitalize()}{name}"
        lines[line_number - 1] = re.sub(
            rf"(?<![\w:]){name}(?![\w])(?! as)", f"{name} as {alias}", lines[line_number - 1], count=1
        )
        LIB.write_text("\n".join(lines))
        print(f"{module}::{name} -> {alias}")
    print("gave up after twelve rounds")
    return 1


if __name__ == "__main__":
    sys.exit(main())
