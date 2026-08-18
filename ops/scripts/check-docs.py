#!/usr/bin/env python3
"""Check the documentation the way the compiler checks the code.

Prose rots differently from code: nothing fails when a section is renamed and
six cross-references quietly start pointing at nothing. The docs here are a
deliverable, so they get a gate.

Four checks, all cheap:

  1. every relative link resolves to a file that exists
  2. every '#anchor' resolves to a heading that exists in that file
  3. every document under docs/ is listed in the README index
  4. every ```mermaid block is closed, non-empty and declares a diagram type

Run directly, or via `just docs-check` (included in `just test`).
"""

from __future__ import annotations

import re
import sys
import pathlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DOCS = ROOT / "docs"
README = ROOT / "README.md"

LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
HEADING = re.compile(r"^#{1,6}\s+(.*)$", re.M)
EXTERNAL = ("http://", "https://", "mailto:")
MERMAID = re.compile(r"```mermaid\n(.*?)```", re.S)
# The types actually used here. Adding a new one is a deliberate act, so it
# belongs in this list rather than being waved through by a looser pattern.
MERMAID_KINDS = {"flowchart", "graph", "sequenceDiagram", "stateDiagram-v2"}


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

    diagrams = check_mermaid(documents, problems)

    if problems:
        print("documentation problems:", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        return 1

    print(
        f"    {len(documents)} documents, every link and anchor resolves"
        f", {diagrams} diagrams well-formed"
    )
    return 0


def check_mermaid(documents: list[pathlib.Path], problems: list[str]) -> int:
    """Structural check on ```mermaid blocks.

    Deliberately not a parse. Running the real grammar needs Node, mermaid and
    a DOM, and making `just test` depend on all three to lint four diagrams is a
    bad trade. This catches what actually rots -- an unclosed fence, an empty
    block, a diagram type nobody declared -- and leaves grammar to the renderer.

    To check the grammar properly, which is worth doing when adding a diagram:

        npm install mermaid jsdom
        node -e "..."   # mermaid.parse() on each block

    GitHub renders these natively, so a broken diagram shows as an error box on
    the page rather than failing anything here.
    """
    count = 0
    for doc in documents:
        text = doc.read_text()
        if text.count("```mermaid") != len(MERMAID.findall(text)):
            problems.append(f"{doc.name}: an unclosed ```mermaid fence")
            continue
        for block in MERMAID.findall(text):
            count += 1
            body = block.strip()
            if not body:
                problems.append(f"{doc.name}: empty mermaid block")
                continue
            kind = body.split()[0].split("\n")[0]
            if kind not in MERMAID_KINDS:
                problems.append(
                    f"{doc.name}: mermaid block starts with '{kind}', "
                    f"expected one of {sorted(MERMAID_KINDS)}"
                )
    return count


if __name__ == "__main__":
    sys.exit(main())
