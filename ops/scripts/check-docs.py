#!/usr/bin/env python3
"""Check the documentation the way the compiler checks the code.

Prose rots differently from code: nothing fails when a section is renamed and
six cross-references quietly start pointing at nothing. The docs here are a
deliverable, so they get a gate.

Three checks, all cheap:

  1. every relative link resolves to a file that exists
  2. every '#anchor' resolves to a heading that exists in that file
  3. every document under docs/ is listed in the README index

Run directly, or via `just docs-check` (included in `just test`).
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"
README = ROOT / "README.md"

LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
HEADING = re.compile(r"^#{1,6}\s+(.*)$", re.M)
EXTERNAL = ("http://", "https://", "mailto:")


def anchor(heading: str) -> str:
    """GitHub's slug: lowercase, drop punctuation, spaces to hyphens.

    Accented characters survive, which matters because these documents are in
    Portuguese and half the headings have one.
    """
    slug = heading.strip().lower()
    slug = re.sub(r"[`*\[\]()]", "", slug)
    slug = re.sub(r"[^\w\s-]", "", slug, flags=re.UNICODE)
    return slug.strip().replace(" ", "-")


def main() -> int:
    documents = sorted(DOCS.glob("*.md")) + [README]
    anchors = {
        doc.name: {anchor(h) for h in HEADING.findall(doc.read_text())}
        for doc in documents
    }

    problems: list[str] = []

    for doc in documents:
        text = doc.read_text()
        for target in LINK.findall(text):
            if target.startswith(EXTERNAL) or target.startswith("#") and not target[1:]:
                continue
            path_part, _, fragment = target.partition("#")

            if path_part:
                resolved = (doc.parent / path_part).resolve()
                if not resolved.exists():
                    problems.append(f"{doc.relative_to(ROOT)}: no such file -> {target}")
                    continue
                name = resolved.name
            else:
                name = doc.name

            if fragment and name in anchors and fragment not in anchors[name]:
                problems.append(f"{doc.relative_to(ROOT)}: no such heading -> {target}")

    # Every document must be reachable from the index, or nobody will find it.
    index = README.read_text()
    for doc in sorted(DOCS.glob("*.md")):
        if f"docs/{doc.name}" not in index:
            problems.append(f"README.md: {doc.name} is not in the documentation index")

    if problems:
        print("documentation problems:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1

    print(f"    {len(documents)} documents, every link and anchor resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
