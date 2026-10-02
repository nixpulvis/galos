#!/usr/bin/env bash
# Profile the running map through a scripted camera, and say where the time
# went.
#
# Builds the map with `--features tracy`, flies each scenario with the
# `GALOS_SHOT` driver (`src/dev/shot.rs`), records it with `tracy-capture`,
# and reads the trace back with `tracy-csvexport`. Per scenario it prints the
# frame times (p50, p90, p99, max) and what the slowest tenth of frames spent,
# by zone; see the README's Profiling for how to read it.
#
#   galos_map/profile.sh                            # every scenario
#   galos_map/profile.sh -f 240 -z build_glow out pan
#   galos_map/profile.sh -r out                     # read the traces kept in -o
#   galos_map/profile.sh -o /tmp/before ...; galos_map/profile.sh -o /tmp/after ...
#   galos_map/profile.sh -c /tmp/before /tmp/after  # the two, scenario by scenario
#
# Needs Tracy 0.13.1 on the path (`brew install tracy`): the client's
# protocol is checked when the capture connects.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
binary="$root/target/release/galos_map"
index="$root/.index/full"
frames=360
out=/tmp/galos-profile
top=12
zones=""
reading=false
compare=()

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
  echo "Options: -i INDEX  -f FRAMES  -o OUT  -z ZONE (repeatable)  -t TOP"
  echo "         -r (read, fly nothing)  -c BEFORE AFTER (compare)"
  echo "Scenarios: still out in pan turn hide galaxy galaxypan"
  exit "${1:-0}"
}

# What a scenario does to the camera: where it stands back from, and how it
# moves a frame. See `src/dev/shot.rs` for what each variable means.
scenario() {
  case $1 in
    still) echo "GALOS_SHOT_BACK=30000" ;;
    out) echo "GALOS_SHOT_BACK=1500 GALOS_SHOT_ZOOM=0.012" ;;
    in) echo "GALOS_SHOT_BACK=60000 GALOS_SHOT_ZOOM=-0.012" ;;
    pan) echo "GALOS_SHOT_BACK=3000 GALOS_SHOT_PAN=25" ;;
    turn) echo "GALOS_SHOT_BACK=3000 GALOS_SHOT_SPIN=0.01" ;;
    # Standing still as `still` does, where hiding No state was measured, the
    # key's No state hidden once the view has loaded.
    hide) echo "GALOS_SHOT_BACK=30000 GALOS_SHOT_HIDE=state:0,27 GALOS_SHOT_HIDE_AT=120" ;;
    # The galaxy seen whole from above, as `media.sh` takes it, standing
    # still and then sliding along x.
    galaxy) echo "GALOS_SHOT_Z=25000 GALOS_SHOT_BACK=115000 GALOS_SHOT_PITCH=-1.55" ;;
    galaxypan) echo "GALOS_SHOT_Z=25000 GALOS_SHOT_BACK=115000 GALOS_SHOT_PITCH=-1.55 GALOS_SHOT_PAN=150" ;;
    *) echo "no scenario $1; there are still out in pan turn hide galaxy galaxypan" >&2; exit 2 ;;
  esac
}

while getopts "i:f:o:z:t:rc:h" flag; do
  case $flag in
    i) index=$OPTARG ;;
    f) frames=$OPTARG ;;
    o) out=$OPTARG ;;
    z) zones="$zones $OPTARG" ;;
    t) top=$OPTARG ;;
    r) reading=true ;;
    c) compare=("$OPTARG") ;;
    h) usage ;;
    *) usage 2 ;;
  esac
done
shift $((OPTIND - 1))
if [ ${#compare[@]} -eq 1 ]; then
  [ $# -ge 1 ] || usage 2
  compare+=("$1")
  shift
fi
scenarios=("$@")
[ ${#scenarios[@]} -gt 0 ] || scenarios=(still out in pan turn hide galaxy galaxypan)

# Fly one scenario and capture it to `$out/<name>.tracy`.
fly() {
  local name=$1 trace="$out/$1.tracy"
  rm -f "$trace"
  # A map left over from an earlier run holds the port and keeps the capture
  # from ever connecting to this one.
  pkill -f "$binary" || true
  # The map first and the capture after it: a capture already listening can
  # lose the handshake while the map starts. See the README.
  # shellcheck disable=SC2046 # the scenario's variables, split on purpose
  env GALOS_SHOT="$out/$name.png" GALOS_SHOT_Z=0 GALOS_SHOT_WAIT="$frames" \
    $(scenario "$name") "$binary" -i "$index" >"$out/$name.log" 2>&1 &
  local map=$!
  sleep 2
  tracy-capture -o "$trace" -f >/dev/null 2>&1 || true
  wait "$map" || true
  [ -f "$trace" ] || { echo "$name: no trace; see $out/$name.log" >&2; exit 1; }
}

# Frame times, and what the slowest tenth of frames spent by zone.
#
# A frame is the time between two runs of the shot driver, which runs once an
# `Update`. Bevy runs systems on whichever thread is free, so they are not
# told apart by thread: what is reported apart is the map's own background
# tasks (`cell payloads`, `build batch`, ...), which a frame waits on only
# through what it waits for, summed over the threads they ran on. Zones nest,
# so a zone's time includes the zones inside it.
report() {
  local label=$1 trace=$2 events windows
  events=$(mktemp) windows=$(mktemp)
  # In time order, so each event falls in the frame the last driver run
  # before it opened.
  tracy-csvexport -u -s $'\t' "$trace" | tail -n +2 |
    LC_ALL=C sort -t $'\t' -k4,4n >"$events"

  # The frames first, off the driver's own runs: their times, and which are
  # the slowest tenth. What is left is a pass over the events that only
  # counts the ones inside those.
  LC_ALL=C awk -F'\t' -v label="$label" -v windows="$windows" '
    function sort(a, n,   i, j, v) {  # insertion sort, ascending
      for (i = 2; i <= n; i++) { v = a[i]; for (j = i - 1; j > 0 && a[j] > v; j--) a[j + 1] = a[j]; a[j + 1] = v }
    }
    function at(a, n, f,   i) { i = int(n * f) + 1; if (i > n) i = n; return a[i] }
    $1 == "system{name=\"galos_map::dev::shot::capture\"}" { starts[++n] = $4 }
    END {
      frames = n - 1
      if (frames < 1) { print label ": no frames"; exit 1 }
      for (i = 1; i <= frames; i++) { time[i] = (starts[i + 1] - starts[i]) / 1e6; ordered[i] = time[i]; total += time[i] }
      sort(ordered, frames)
      cut = at(ordered, frames, 0.9)
      printf "%s: %d frames over %.1f s, p50 %.1f p90 %.1f p99 %.1f max %.1f ms\n", \
        label, frames, total / 1e3, at(ordered, frames, 0.5), cut, at(ordered, frames, 0.99), ordered[frames]
      printf "frames\t%d\t%s\t%s\n", frames, starts[1], starts[n] >windows
      for (i = 1; i <= frames; i++) if (time[i] >= cut) printf "%s\t%s\n", starts[i], starts[i + 1] >windows
    }' <(grep -F 'system{name="galos_map::dev::shot::capture"}' "$events") ||
    { rm -f "$events" "$windows"; return; }

  LC_ALL=C awk -F'\t' -v top="$top" -v zones="$zones" '
    function zone(name,   rest, i) {
      if (substr(name, 1, 13) == "system{name=\"") {
        rest = substr(name, 14)
        return "system " substr(rest, 1, index(rest, "\"") - 1)
      }
      i = index(name, "{")
      return i ? substr(name, 1, i - 1) : name
    }
    BEGIN {
      nz = split(zones, asked, " ")
      split("schedule|update|main app|sub app|frame|multithreaded executor|system galos_map::dev::shot::capture", list, "|")
      for (i in list) skip[list[i]] = 1
      # The map'"'"'s own task zones: work on the pools a frame does not wait
      # on, except through what it waits for.
      split("cell payloads|build batch|index read|refresh poll|route search|name search|stop lookup|bodies read|region cells", list, "|")
      for (i in list) task[list[i]] = 1
    }
    FNR == NR {
      if ($1 == "frames") { frames = $2; first = $3; last = $4 } else { from[++nslow] = $1; to[nslow] = $2 }
      next
    }
    {
      t = $4 + 0
      if (t < first || t >= last) next
      if (nz) for (z = 1; z <= nz; z++) if (index($1, asked[z])) each[z] += $5 / 1e6
      # Events come in time order and so do the windows: move on past the
      # ones this event is beyond.
      while (w < nslow && t >= to[w + 1]) w++
      if (w >= nslow || t < from[w + 1]) next
      name = zone($1)
      if (name in skip) next
      spent[((name in task) ? "tasks" : "frame") "\t" name] += $5 / 1e6
    }
    END {
      printf "  the slowest tenth, %d frames, per frame:\n", nslow
      split("frame tasks", groups, " ")
      for (g = 1; g <= 2; g++) {
        m = 0
        for (k in spent) { split(k, kk, "\t"); if (kk[1] == groups[g]) { m++; keys[m] = kk[2]; vals[m] = spent[k] } }
        if (m == 0) continue
        printf "    %s:\n", (groups[g] == "tasks" ? "background tasks (summed over the threads they ran on)" : "systems and their zones")
        for (s = 1; s <= top && s <= m; s++) {  # the largest `top`, by selection
          best = s
          for (j = s + 1; j <= m; j++) if (vals[j] > vals[best]) best = j
          v = vals[s]; vals[s] = vals[best]; vals[best] = v
          k = keys[s]; keys[s] = keys[best]; keys[best] = k
          shown = keys[s]; sub(/^system /, "", shown)
          printf "      %8.2f ms  %s\n", vals[s] / nslow, shown
        }
        delete keys; delete vals
      }
      for (z = 1; z <= nz; z++) printf "  %s: %.2f ms a frame\n", asked[z], each[z] / frames
    }' "$windows" "$events"
  rm -f "$events" "$windows"
}

if [ ${#compare[@]} -eq 2 ]; then
  for name in "${scenarios[@]}"; do
    for side in "${compare[@]}"; do
      [ -f "$side/$name.tracy" ] && report "$name ($side)" "$side/$name.tracy"
    done
  done
  exit 0
fi

mkdir -p "$out"
if ! $reading; then
  (cd "$root" && cargo build --release -q -p galos_map --features tracy)
fi
for name in "${scenarios[@]}"; do
  scenario "$name" >/dev/null
  $reading || fly "$name"
  report "$name" "$out/$name.tracy"
done
