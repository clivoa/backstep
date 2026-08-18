#!/usr/bin/env python3
"""Burn a session's telemetry into its recording.

A clean recording of a rollback session is, by design, unremarkable: the whole
point of rollback is that the correction is invisible. Watching one proves the
game ran, and nothing else.

What makes it documentation is putting the session log next to it. The JSONL
already carries every frame's event -- advanced, rolled back, stalled -- so this
generates a subtitle track from it and asks ffmpeg to burn it in. The result
shows, on the exact frame it happened:

  * a running frame / rollback / depth / RTT readout
  * a marker the moment a rollback fires
  * a bar across the screen while the session is stalled

So you can watch a fight that looks perfectly smooth while the overlay counts a
thousand corrections going past.

    ./ops/scripts/annotate-video.py session.mp4 session.jsonl out.mp4

The subtitle format is ASS rather than drawtext filters, because one
`-vf ass=` is cheaper than several thousand drawtext expressions, and because
an .ass file can be inspected on its own when the overlay looks wrong.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

# The recorder scales 3x; the overlay is authored in that space.
VIDEO_W = 960
VIDEO_H = 672
# The telemetry goes in a band *above* the picture rather than on top of it.
# Drawn over the game it collided with the game's own HUD -- health bars and
# round timer live in exactly the same corner -- and both became hard to read.
# A band costs a little height and keeps the emulator's output untouched, which
# also means the video still shows what the player actually saw.
BAND_H = 96
PLAY_RES_X = VIDEO_W
PLAY_RES_Y = VIDEO_H + BAND_H
FPS = 60.0

ASS_HEADER = f"""[Script Info]
ScriptType: v4.00+
PlayResX: {PLAY_RES_X}
PlayResY: {PLAY_RES_Y}
WrapStyle: 2
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, OutlineColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: hud,monospace,21,&H00FFFFFF,&H00000000,&H00000000,0,0,1,0,0,7,14,14,8,1
Style: title,monospace,23,&H0000D7FF,&H00000000,&H00000000,1,0,1,0,0,9,14,14,8,1
Style: note,monospace,24,&H0000D7FF,&H00000000,&H80000000,1,0,3,2,0,9,14,14,{BAND_H + 12},1
Style: alarm,monospace,30,&H004040FF,&H00000000,&H80000000,1,0,3,2,0,5,14,14,14,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
"""


def ts(seconds: float) -> str:
    """ASS timestamp: h:mm:ss.cc"""
    seconds = max(0.0, seconds)
    h = int(seconds // 3600)
    m = int((seconds % 3600) // 60)
    s = seconds % 60
    return f"{h}:{m:02d}:{s:05.2f}"


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    video = Path(sys.argv[1])
    log = Path(sys.argv[2])
    out = Path(sys.argv[3])
    label = sys.argv[4] if len(sys.argv) > 4 else ""

    for tool in ("ffmpeg",):
        if not shutil.which(tool):
            print(f"{tool} is not on PATH", file=sys.stderr)
            return 1
    if not video.exists():
        print(f"no recording at {video}", file=sys.stderr)
        return 1
    if not log.exists():
        print(f"no session log at {log}", file=sys.stderr)
        return 1

    # --- read the session -------------------------------------------------
    info: dict = {}
    rollbacks: list[tuple[int, int]] = []  # (frame, depth)
    stalls: list[int] = []
    metrics: list[dict] = []
    desync_at: int | None = None
    first_frame: int | None = None
    last_frame = 0

    for line in log.open():
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            continue  # a session killed mid-write leaves one short line
        kind = rec.get("record")
        if kind == "session_start":
            info = rec.get("info", {})
        elif kind == "metrics":
            metrics.append(rec)
        elif kind == "session":
            event = rec.get("event")
            frame = rec.get("frame", 0)
            if event == "advanced":
                if first_frame is None:
                    first_frame = frame
                last_frame = max(last_frame, frame)
            elif event == "rolled_back":
                # `frame` is absent on this event; it carries from/to instead.
                rollbacks.append((rec.get("from", 0), rec.get("depth", 0)))
            elif event == "stalled":
                stalls.append(frame)
            elif event == "desync":
                desync_at = frame

    if first_frame is None:
        print("the log contains no advanced frames", file=sys.stderr)
        return 1

    # The recording starts at the first presented frame, so video time zero is
    # that frame. Everything below is expressed relative to it.
    def at(frame: int) -> float:
        return max(0.0, (frame - first_frame) / FPS)

    # --- build the overlay ------------------------------------------------
    events: list[str] = []

    def dialogue(start: float, end: float, style: str, text: str) -> None:
        events.append(
            f"Dialogue: 0,{ts(start)},{ts(end)},{style},,0,0,0,,{text}"
        )

    # A once-per-second readout, driven by the metrics records the session
    # already writes. Cumulative counters, so the numbers on screen are the
    # numbers the report will show.
    for i, m in enumerate(metrics):
        local = m.get("local", {}) or {}
        link = m.get("link", {}) or {}
        frame = m.get("frame", 0)
        elapsed_ms = m.get("elapsed_ms", 0) or 1
        start = at(frame)
        end = at(metrics[i + 1].get("frame", frame)) if i + 1 < len(metrics) else start + 1.0
        if end <= start:
            end = start + 1.0

        presented = local.get("frames_presented", 0)
        resimulated = local.get("frames_resimulated", 0)
        rollbacks_so_far = local.get("rollbacks", 0)
        # Derived the same way the report derives them, so the overlay and
        # summary.csv cannot disagree.
        fps = presented / (elapsed_ms / 1000.0)
        overhead = (resimulated / presented * 100.0) if presented else 0.0
        sent = link.get("packets_sent", 0)
        got = link.get("unique_received", 0)
        highest = link.get("highest_sequence", 0)
        expected = highest + 1
        loss = ((expected - got) / expected * 100.0) if expected > 0 else 0.0

        hud = (
            f"frame {frame:>6}   rollbacks {rollbacks_so_far:>5}   "
            f"depth now {m.get('prediction_depth', 0)}   max {local.get('max_rollback_depth', 0)}\\N"
            f"fps {fps:>5.2f}   resim {overhead:>5.1f}%   stalls {local.get('stalls', 0)}\\N"
            f"rtt {link.get('srtt_micros', 0) / 1000.0:>5.1f} ms   "
            f"jitter {link.get('rttvar_micros', 0) / 1000.0:>4.2f} ms   "
            f"loss {max(0.0, loss):>4.2f}%   sent {sent}"
        )
        dialogue(start, end, "hud", hud)

    # A marker on the exact frame a rollback fired. Deliberately brief: at
    # depth 2 the correction is two frames of work, and the flash should feel
    # like that rather than like an alarm.
    for frame, depth in rollbacks:
        start = at(frame)
        dialogue(start, start + 0.25, "note", f"ROLLBACK -{depth}")

    # Stalls are the visible failure, so they get the loud style and last as
    # long as they actually lasted.
    if stalls:
        run_start = stalls[0]
        prev = stalls[0]
        for f in stalls[1:] + [None]:
            if f is None or f > prev + 1:
                dialogue(
                    at(run_start),
                    at(prev) + 1 / FPS,
                    "alarm",
                    f"STALLED {prev - run_start + 1} frames",
                )
                if f is not None:
                    run_start = f
            if f is not None:
                prev = f

    if desync_at is not None:
        dialogue(at(desync_at), at(last_frame), "alarm", "DESYNC")

    # A standing caption naming the scenario, so a video pulled out of context
    # still says what it is.
    if not label:
        label = (
            f"{info.get('simulation', '?')}  profile={info.get('profile', '?')}  "
            f"{info.get('player', '?')}"
        )
    dialogue(0.0, at(last_frame), "title", label.replace(",", ";"))

    ass = Path(str(out) + ".ass")
    ass.write_text(ASS_HEADER + "\n".join(events) + "\n")

    # --- burn it in -------------------------------------------------------
    cmd = [
        "ffmpeg",
        "-hide_banner",
        "-loglevel",
        "error",
        "-y",
        "-i",
        str(video),
        "-vf",
        # Pad first, then draw: the subtitle renderer is told the padded
        # geometry, so band coordinates and video coordinates agree.
        f"pad={VIDEO_W}:{VIDEO_H + BAND_H}:0:{BAND_H}:black,ass={ass}",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "20",
        "-pix_fmt",
        "yuv420p",
        str(out),
    ]
    result = subprocess.run(cmd)
    if result.returncode != 0:
        return result.returncode

    ass.unlink(missing_ok=True)
    size = out.stat().st_size
    print(
        f"    {out}  ({size / 1e6:.1f} MB, {len(rollbacks)} rollbacks, "
        f"{len(stalls)} stalled frames)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
