#!/usr/bin/env python3
"""Run the ES|QL queries in ops/elastic/queries.esql and print their tables.

    ./ops/scripts/elastic-esql.py              # every query
    ./ops/scripts/elastic-esql.py distance     # one, by name
    ./ops/scripts/elastic-esql.py --list       # names and descriptions
    ./ops/scripts/elastic-esql.py --markdown   # tables ready to paste into docs

The queries live in a plain `.esql` file rather than inside this script so they
can be copied straight into Kibana's Discover in ES|QL mode, where the same
query gets a chart beside the table. This runner exists so they are also
executable, and so documentation built from them can be re-derived rather than
transcribed by hand.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[2]
QUERIES = ROOT / "ops" / "elastic" / "queries.esql"
HEADER = re.compile(r"^--\s*(\w[\w-]*)\s*:\s*(.*)$")


def parse(path: pathlib.Path) -> list[tuple[str, str, str]]:
    """Split the file into (name, description, query) triples."""
    blocks: list[tuple[str, str, str]] = []
    name = description = None
    body: list[str] = []

    for line in path.read_text().splitlines():
        match = HEADER.match(line)
        if match:
            if name:
                blocks.append((name, description or "", "\n".join(body).strip()))
            name, description = match.group(1), match.group(2)
            body = []
            continue
        if name is None:
            continue  # the file's own preamble
        # `--` inside a block is a comment on the query, not a new block.
        if line.startswith("--"):
            continue
        body.append(line)

    if name:
        blocks.append((name, description or "", "\n".join(body).strip()))
    return [b for b in blocks if b[2]]


def run(url: str, query: str, fmt: str) -> str:
    request = urllib.request.Request(
        f"{url}/_query?format={fmt}",
        data=json.dumps({"query": query}).encode(),
        method="POST",
        headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            return response.read().decode()
    except urllib.error.HTTPError as e:
        detail = e.read().decode()
        try:
            detail = json.loads(detail)["error"]["reason"]
        except Exception:
            detail = detail[:400]
        return f"!!! query failed: {detail}"
    except urllib.error.URLError as e:
        raise SystemExit(f"cannot reach {url}: {e.reason}. Is `just elastic-up` running?") from e


def as_markdown(csv_text: str) -> str:
    """Turn ES|QL's CSV output into a Markdown table."""
    import csv
    import io

    rows = list(csv.reader(io.StringIO(csv_text)))
    if not rows:
        return "_no rows_"
    head, *body = rows
    out = ["| " + " | ".join(head) + " |",
           "|" + "|".join("---" for _ in head) + "|"]
    for row in body:
        out.append("| " + " | ".join(row) + " |")
    return "\n".join(out)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("name", nargs="*", help="queries to run (default: all)")
    ap.add_argument("--url", default="http://127.0.0.1:9200")
    ap.add_argument("--list", action="store_true", help="list names and stop")
    ap.add_argument("--markdown", action="store_true", help="emit Markdown tables")
    args = ap.parse_args()

    if not QUERIES.exists():
        print(f"!!! {QUERIES} is missing", file=sys.stderr)
        return 2

    blocks = parse(QUERIES)
    if args.list:
        for name, description, _ in blocks:
            print(f"  {name:<18}{description}")
        return 0

    wanted = set(args.name)
    if wanted:
        unknown = wanted - {b[0] for b in blocks}
        if unknown:
            print(f"!!! no such query: {', '.join(sorted(unknown))}", file=sys.stderr)
            return 2
        blocks = [b for b in blocks if b[0] in wanted]

    failed = 0
    for name, description, query in blocks:
        if args.markdown:
            print(f"\n### `{name}` -- {description}\n")
            print("```esql")
            print(query)
            print("```\n")
            print(as_markdown(run(args.url, query, "csv")))
        else:
            print(f"\n=== {name}: {description}")
            output = run(args.url, query, "txt")
            print(output)
            if output.startswith("!!!"):
                failed += 1

    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
