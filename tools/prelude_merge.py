#!/usr/bin/env python3
"""Add a prelude line for every module that does not have one yet.

A lane writes its own prelude snippet beside its module, but that file lives in /tmp and a
machine restart takes it away — which leaves a module that compiles and a prelude that never
mentions it, so nothing outside the module can reach its types. This reads each module's
public types and writes the line itself, then tools/prelude_alias.py settles any name the
crate has already bound elsewhere.
"""

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "cinnaloader" / "sdk" / "src"
LIB = SRC / "lib.rs"
DECLARATION = re.compile(r"^pub (?:struct|enum|trait|type) ([A-Z]\w*)", re.M)


def public_types(module: str) -> list[str]:
    text = (SRC / f"{module}.rs").read_text()
    names = []
    for name in DECLARATION.findall(text):
        if name not in names:
            names.append(name)
    return names


def main(argv: list[str]) -> int:
    modules = argv or sorted(
        path.stem
        for path in SRC.glob("*.rs")
        if path.stem not in {"lib", "level", "prelude"}
    )
    body = LIB.read_text()
    if not body.endswith("\n"):
        body += "\n"
    added = 0
    for module in modules:
        if f"pub use crate::{module}::{{" in body:
            continue
        names = public_types(module)
        rendered = [f"    pub use crate::{module}::{{", "        self as " + module + ","]
        line = "        "
        for name in names:
            piece = name + ","
            if len(line) + len(piece) > 96:
                rendered.append(line.rstrip())
                line = "        "
            line += piece + " "
        if line.strip():
            rendered.append(line.rstrip())
        rendered.append("    };")
        block = "\n".join(rendered) + "\n"
        body = body.replace("    pub use crate::draw::{", block + "    pub use crate::draw::{", 1)
        added += 1
    LIB.write_text(body)
    print(f"added {added} prelude line(s)")
    if added:
        subprocess.run([sys.executable, str(ROOT / "tools" / "prelude_alias.py")], check=False)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
