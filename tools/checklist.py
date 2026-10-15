#!/usr/bin/env python3
"""Flip marks in checklist.md for a whole wishlist section.

    python3 tools/checklist.py --section 4 --done
    python3 tools/checklist.py --section 4 --partial
    python3 tools/checklist.py --section 4 --na
    python3 tools/checklist.py --list          # per-section counts

Marks: [ ] todo, [~] partial, [x] done, [-] not applicable.
"""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHECKLIST = ROOT / "checklist.md"
MARKS = {"done": "x", "partial": "~", "na": "-", "todo": " "}


def sections(text):
    """[(number, title, start_line, end_line)]"""
    found = []
    for index, line in enumerate(text.split("\n")):
        match = re.match(r"^## (\d+)\. (.+)$", line)
        if match:
            found.append([int(match.group(1)), match.group(2), index])
    for position, entry in enumerate(found):
        entry.append(found[position + 1][2] if position + 1 < len(found) else None)
    return found


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--section", type=int)
    parser.add_argument("--done", action="store_true")
    parser.add_argument("--partial", action="store_true")
    parser.add_argument("--na", action="store_true")
    parser.add_argument("--todo", action="store_true")
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--only", help="flip only entries whose text contains this")
    args = parser.parse_args()

    lines = CHECKLIST.read_text().split("\n")
    found = sections("\n".join(lines))

    if args.list or not any([args.done, args.partial, args.na, args.todo]):
        for number, title, start, end in found:
            body = "\n".join(lines[start : end or len(lines)])
            counts = {
                mark: len(re.findall(rf"^- \[{mark}\]", body, re.M))
                for mark in [" ", "~", "x", "-"]
            }
            total = sum(counts.values())
            print(
                f"{number:3} {title[:44]:46} total {total:4} todo {counts[' ']:4} "
                f"partial {counts['~']:3} done {counts['x']:4} n/a {counts['-']:3}"
            )
        if args.list:
            return

    if args.section is None:
        sys.exit("--section is required to flip marks")

    mark = next(
        mark
        for mark, flag in [
            ("x", args.done),
            ("~", args.partial),
            ("-", args.na),
            (" ", args.todo),
        ]
        if flag
    )
    flipped = 0
    for number, _title, start, end in found:
        if number != args.section:
            continue
        for index in range(start, end or len(lines)):
            if not lines[index].startswith("- ["):
                continue
            if args.only and args.only not in lines[index]:
                continue
            lines[index] = f"- [{mark}]{lines[index][5:]}"
            flipped += 1
    CHECKLIST.write_text("\n".join(lines))
    print(f"section {args.section}: flipped {flipped} entries to [{mark}]")


if __name__ == "__main__":
    main()
