#!/usr/bin/env python3
"""Summarise a tuning sweep as a Markdown table.

Reads the `session_end` record from every JSONL in a directory and prints one
row per configuration. Only P1 is shown by default: on a symmetric link the two
peers agree closely, and one row per configuration is what a comparison needs.

    ./ops/scripts/tuning-table.py artifacts/tuning/logs
    ./ops/scripts/tuning-table.py artifacts/tuning/logs --player p2
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys

# Written by ops/scripts/tuning-sweep.sh as `--mode tune-<name>`, and it lands
# in the filename. Anything else in the directory is ignored rather than
# guessed at.
NAME = re.compile(r"^\d+-(?P<sim>\w+)-(?P<profile>\w+)-(?P<player>p\d)-tune-(?P<config>[\w-]+)\.jsonl$")


def session_end(path: pathlib.Path) -> dict | None:
    """Return the last `session_end` record, or None if the run did not finish.

    Read line by line: these logs reach hundreds of megabytes for a long
    session, and only the final record is wanted.
    """
    found = None
    with path.open() as handle:
        for line in handle:
            try:
                record = json.loads(line)
            except json.JSONDecodeError:
                continue  # a truncated tail is expected if a peer was killed
            if record.get("record") == "session_end":
                found = record
    return found


def rows(log_dir: pathlib.Path, player: str) -> list[dict]:
    out = []
    for path in sorted(log_dir.glob("*.jsonl")):
        match = NAME.match(path.name)
        if not match or match["player"] != player:
            continue
        end = session_end(path)
        if end is None:
            print(f"!!! {path.name} has no session_end; skipping", file=sys.stderr)
            continue

        local = end["local"]
        info = end["info"]
        simulated = local["frames_presented"] + local["frames_resimulated"]
        seconds = end["elapsed_ms"] / 1000

        out.append(
            {
                "config": match["config"],
                "delay": info["input_delay"],
                "limit": info["prediction_limit"],
                "history": info["state_history"],
                "fps": local["frames_presented"] / seconds,
                "stalls": local["stalls"],
                "rollbacks": local["rollbacks"],
                "depth_max": local["max_rollback_depth"],
                # The real cost of rollback: frames the CPU ran that the player
                # never saw, as a share of the frames they did see.
                "resim_ratio": local["frames_resimulated"] / max(local["frames_presented"], 1),
                "accuracy": 100 * (1 - local["mispredicted_frames"] / max(local["predicted_frames"], 1)),
                "checksums": local["checksums_compared"],
                "simulated": simulated,
            }
        )
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("log_dir", type=pathlib.Path, nargs="?", default=pathlib.Path("artifacts/tuning/logs"))
    parser.add_argument("--player", default="p1", choices=["p1", "p2"])
    args = parser.parse_args()

    if not args.log_dir.is_dir():
        print(f"!!! {args.log_dir} is not a directory", file=sys.stderr)
        return 2

    table = rows(args.log_dir, args.player)
    if not table:
        print(f"!!! no {args.player} tuning logs in {args.log_dir}", file=sys.stderr)
        return 1

    # Input lag is what the player actually feels, and it is the price every
    # other column is being bought with, so it belongs in the table.
    print("| Configuration | Input delay | Limit | History | FPS | Stalls | Rollbacks | Max depth | Re-sim | Accuracy | Input lag |")
    print("|---|---|---|---|---|---|---|---|---|---|---|")
    for row in sorted(table, key=lambda r: (r["delay"], r["limit"])):
        lag = row["delay"] * 1000 / 60
        print(
            f"| {row['config']} | {row['delay']} | {row['limit']} | {row['history']} "
            f"| {row['fps']:.2f} | {row['stalls']} | {row['rollbacks']} | {row['depth_max']} "
            f"| {row['resim_ratio']:.2f}x | {row['accuracy']:.1f}% | {lag:.0f} ms |"
        )

    total = sum(r["checksums"] for r in table)
    print(f"\n{total} checksums compared across {len(table)} configurations.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
