#!/usr/bin/env python3
"""Profile the running map through a scripted camera, and say where the time went.

Builds the map with `--features tracy`, flies each scenario with the
`GALOS_SHOT` driver (`src/dev/shot.rs`), records it with `tracy-capture`, and
reads the trace back with `tracy-csvexport`. What it prints per scenario:

- frame times (p50, p90, p99, max), a frame being the time between two runs
  of the shot driver, which runs once an `Update`;
- what the slowest tenth of frames spent, by thread and by zone, per slow
  frame: bevy's systems and the map's own zones (`cell prefixes`, `build
  batch`, ...). Zones nest — `cell prefixes` is inside `reconcile` — so a
  zone's time is its total and not its own. Rendering runs on the main
  thread in this build, so it holds the frame up with the rest;
- per-frame means of any zones asked for with `--zone`.

Traces are kept in `--out` and can be read again without flying: pass
`--read` and the scenario names. Two runs are compared by passing two trace
directories to `--compare`.

    galos_map/profile.py                           # every scenario
    galos_map/profile.py still out --frames 240    # some of them
    galos_map/profile.py --read out --zone build_glow
    galos_map/profile.py --compare /tmp/before /tmp/after

Needs Tracy 0.13.1 on the path (`brew install tracy`): the client's
protocol is checked when the capture connects. See the README's Profiling.
"""

import argparse
import bisect
import collections
import os
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / "target" / "release" / "galos_map"

# What a scenario does to the camera: where it stands back from, and how it
# moves a frame. See `src/dev/shot.rs` for what each variable means.
SCENARIOS = {
    "still": {"GALOS_SHOT_BACK": "30000"},
    "out": {"GALOS_SHOT_BACK": "1500", "GALOS_SHOT_ZOOM": "0.012"},
    "in": {"GALOS_SHOT_BACK": "60000", "GALOS_SHOT_ZOOM": "-0.012"},
    "pan": {"GALOS_SHOT_BACK": "3000", "GALOS_SHOT_PAN": "25"},
    "turn": {"GALOS_SHOT_BACK": "3000", "GALOS_SHOT_SPIN": "0.01"},
}

# The frame boundary: the shot driver runs once an `Update`.
FRAME = "system galos_map::dev::shot::capture"


def build():
    subprocess.run(
        ["cargo", "build", "--release", "-q", "-p", "galos_map",
         "--features", "tracy"],
        cwd=ROOT, check=True,
    )


def fly(name, index, frames, out):
    """Fly one scenario and capture it to `out/<name>.tracy`."""
    trace = out / f"{name}.tracy"
    trace.unlink(missing_ok=True)
    # A map left over from an earlier run holds the port and keeps the
    # capture from ever connecting to this one.
    subprocess.run(["pkill", "-f", str(BINARY)], check=False)
    env = dict(os.environ)
    env.update({
        "GALOS_SHOT": str(out / f"{name}.png"),
        "GALOS_SHOT_Z": "0",
        "GALOS_SHOT_WAIT": str(frames),
    })
    env.update(SCENARIOS[name])
    # The map first and the capture after it: a capture already listening
    # can lose the handshake while the map starts. See the README.
    log = open(out / f"{name}.log", "w")
    map_ = subprocess.Popen(
        [str(BINARY), "-i", str(index)], env=env, stdout=log,
        stderr=subprocess.STDOUT, cwd=ROOT,
    )
    time.sleep(2)
    subprocess.run(
        ["tracy-capture", "-o", str(trace), "-f"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=600,
    )
    map_.wait(timeout=60)
    if not trace.exists():
        sys.exit(f"{name}: no trace was captured; see {out / (name + '.log')}")
    return trace


def zone(name):
    """A zone's name without its fields: `system{name="x"}` is `system x`."""
    if name.startswith('system{name="'):
        return "system " + name.split('"')[1]
    return name.split("{")[0].strip()


def events(trace):
    """Every zone event in `trace`: (seconds, milliseconds, zone, thread)."""
    lines = subprocess.run(
        ["tracy-csvexport", "-u", "-s", "\t", str(trace)],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()
    head = lines[0].split("\t")
    at, took = head.index("ns_since_start"), head.index("exec_time_ns")
    thread = head.index("thread")
    for line in lines[1:]:
        parts = line.split("\t")
        yield (float(parts[at]) / 1e9, float(parts[took]) / 1e6,
               zone(parts[0]), parts[thread])


# Which thread a zone ran on, as the report groups them: the main thread is
# the one the shot driver runs on, the render thread the one bevy's render
# schedule runs on, and everything else a pool thread whose time does not
# hold a frame up by itself.
THREADS = ("main", "render", "pool")


def read(trace):
    """Frame times, and what each frame spent by thread and zone."""
    all_events = list(events(trace))
    main = {th for _, _, name, th in all_events if name == FRAME}
    render = {th for _, _, name, th in all_events
              if name == "system bevy_render::run_render_schedule"} - main
    starts = sorted(t for t, _, name, _ in all_events if name == FRAME)
    spent = [collections.Counter() for _ in starts]
    for t, took, name, th in all_events:
        at = bisect.bisect_right(starts, t) - 1
        if 0 <= at < len(starts) - 1:
            where = "main" if th in main else "render" if th in render else "pool"
            spent[at][(where, name)] += took
    times = [(b - a) * 1e3 for a, b in zip(starts, starts[1:])]
    return times, spent[: len(times)]


def percentile(values, f):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(len(ordered) * f))]


def report(name, trace, zones, top):
    times, spent = read(trace)
    if not times:
        print(f"{name}: no frames in {trace}")
        return
    print(
        f"{name}: {len(times)} frames over {sum(times) / 1e3:.1f} s, "
        f"p50 {percentile(times, .5):.1f} p90 {percentile(times, .9):.1f} "
        f"p99 {percentile(times, .99):.1f} max {max(times):.1f} ms"
    )
    cut = percentile(times, .9)
    slow = [i for i, t in enumerate(times) if t >= cut]
    worst = collections.Counter()
    for i in slow:
        worst.update(spent[i])
    # The schedule and the executors wrap everything and say nothing.
    wrappers = ("schedule", "update", "main app", "sub app", "frame",
                "multithreaded executor", FRAME)
    print(f"  the slowest tenth, {len(slow)} frames, per frame:")
    for where in THREADS:
        if not any(on == where for on, _ in worst):
            continue
        shown = 0
        print(f"    {where} thread{'s' if where == 'pool' else ''}"
              f"{' (summed over threads)' if where == 'pool' else ''}:")
        for (on, zone_name), took in worst.most_common():
            if on != where or zone_name in wrappers:
                continue
            print(f"      {took / len(slow):8.2f} ms  "
                  f"{zone_name.removeprefix('system ')}")
            shown += 1
            if shown == top:
                break
    for asked in zones:
        per = [
            sum(took for (_, zone_name), took in frame.items()
                if asked in zone_name)
            for frame in spent
        ]
        print(f"  {asked}: {sum(per) / len(per):.2f} ms a frame, "
              f"max {max(per):.2f}")


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("scenarios", nargs="*", default=list(SCENARIOS))
    parser.add_argument("--index", default=str(ROOT / ".index" / "full"))
    parser.add_argument("--frames", type=int, default=360)
    parser.add_argument("--out", default="/tmp/galos-profile")
    parser.add_argument("--read", action="store_true",
                        help="read the traces already in --out; fly nothing")
    parser.add_argument("--zone", action="append", default=[],
                        help="a zone to report per frame, matched by substring")
    parser.add_argument("--top", type=int, default=12)
    parser.add_argument("--compare", nargs=2, metavar=("BEFORE", "AFTER"),
                        help="report two trace directories scenario by scenario")
    args = parser.parse_args()

    if args.compare:
        for name in args.scenarios:
            for side in args.compare:
                trace = Path(side) / f"{name}.tracy"
                if trace.exists():
                    report(f"{name} ({side})", trace, args.zone, args.top)
        return

    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if not args.read:
        build()
    for name in args.scenarios:
        if name not in SCENARIOS:
            sys.exit(f"no scenario {name}; there are {', '.join(SCENARIOS)}")
        trace = out / f"{name}.tracy"
        if not args.read:
            trace = fly(name, args.index, args.frames, out)
        report(name, trace, args.zone, args.top)


if __name__ == "__main__":
    main()
