#!/usr/bin/env bash
# Record the README's pictures and animation of the map.
#
# Builds the map and flies each through the `GALOS_SHOT` driver
# (`src/dev/shot.rs`):
#
#   galaxy  the whole galaxy from above, core at the top    -> galaxy.png
#   local   the stars around Sol, named, from above and tilted -> local.png
#   demo    a flight from the galaxy into the Sol system,     -> demo.gif
#           out to Mars, the names coming on inside 8 light
#           years; recorded a frame at a time and put together
#           by ffmpeg
#
# Written beside this script unless `-o` says elsewhere.
#
#   galos_map/media.sh                 # all three
#   galos_map/media.sh demo            # the animation alone
#   galos_map/media.sh -i .galos_index -o /tmp galaxy local
#
# Needs ffmpeg on the path. The map opens a window of its own for each and
# closes it when done. The frames are what the display shows, so they come
# out black while it is asleep: on macOS the script wakes it and keeps it
# awake with `caffeinate`, and elsewhere that is left to whoever runs it.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
binary="$root/target/release/galos_map"
index="$root/.index/full"
out="$here"

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
  echo "Options: -i INDEX  -o OUT"
  echo "Targets: galaxy local demo"
  exit "${1:-0}"
}

while getopts "i:o:h" flag; do
  case $flag in
    i) index=$OPTARG ;;
    o) out=$OPTARG ;;
    h) usage ;;
    *) usage 2 ;;
  esac
done
shift $((OPTIND - 1))
targets=("$@")
[ ${#targets[@]} -gt 0 ] || targets=(galaxy local demo)
for target in "${targets[@]}"; do
  case $target in
    galaxy | local | demo) ;;
    *) echo "no target $target; there are galaxy local demo" >&2; exit 2 ;;
  esac
done
command -v ffmpeg >/dev/null || { echo "ffmpeg is not on the path" >&2; exit 1; }
[ -f "$index/index.bin" ] || { echo "$index: not a built index" >&2; exit 1; }

work=$(mktemp -d "${TMPDIR:-/tmp}/galos-media.XXXXXX")
trap 'rm -rf "$work"' EXIT

# Where the camera stands, all from galactic north, +Y: negative pitch is
# the eye above the plane, and a half turn of yaw puts +Z back at the top,
# which leaves +X on the right the way the game draws it. The galaxy is held
# from 115,000 light years over a point short of the core, looking straight
# down with the spyglass opened to take in all of it rather than the sphere
# the zoom would give it; the flight starts there. `local` is Sol from 80
# light years, pitched off straight down and turned off square so the rose
# shows both. `inner` is the same angle from 8e-5 light years, five AU: the
# Sun and its planets out to Mars, which is where the flight ends.
galaxy_pose=(GALOS_SHOT_Z=25000 GALOS_SHOT_BACK=115000 GALOS_SHOT_PITCH=-1.55
  GALOS_SHOT_YAW=3.1416 GALOS_SHOT_REACH=60000)
local_pose=(GALOS_SHOT_Z=0 GALOS_SHOT_BACK=80 GALOS_SHOT_PITCH=-1.15
  GALOS_SHOT_YAW=3.4916)
inner_pose=(GALOS_SHOT_TO_Z=0 GALOS_SHOT_TO_BACK=0.00008
  GALOS_SHOT_TO_PITCH=-1.15 GALOS_SHOT_TO_YAW=3.4916)

# The display on, and kept on: `-u` wakes it, and `-d` holds it awake while
# the map runs.
awake=()
if command -v caffeinate >/dev/null; then
  caffeinate -u -t 2
  awake=(caffeinate -d)
fi

# Fly the map with the `GALOS_SHOT_*` variables in the arguments, and check it
# left `$1` behind; see `src/dev/shot.rs` for each variable. The names are
# off unless the arguments turn them back on.
fly() {
  local made=$1 log="$work/map.log"
  shift
  if ! env GALOS_SHOT_NAMES=off "$@" ${awake[@]+"${awake[@]}"} "$binary" \
    -i "$index" >"$log" 2>&1 || [ ! -e "$made" ]; then
    echo "the map left no $made; its log:" >&2
    cat "$log" >&2
    exit 1
  fi
}

# One frame at 1280 by 720 points and one and a half pixels a point, after
# four hundred for the view to load.
still() {
  local name=$1
  shift
  fly "$work/$name.png" GALOS_SHOT="$work/$name.png" GALOS_SHOT_WAIT=400 \
    GALOS_SHOT_WIDTH=1280 GALOS_SHOT_HEIGHT=720 GALOS_SHOT_SCALE=1.5 "$@"
  cp "$work/$name.png" "$out/$name.png"
  echo "$out/$name.png"
}

# Named apart from the targets, `local` being a bash builtin.
take_galaxy() { still galaxy "${galaxy_pose[@]}"; }
take_local() { still local "${local_pose[@]}" GALOS_SHOT_NAMES=on; }

# Held at the galaxy for three hundred frames while it loads, then eighteen
# hundred frames down to the planets with every eighteenth kept: a hundred
# and one at ten a second, with a second on the galaxy before and two on the
# planets after. The frames between the kept ones are what the map loads the
# next view in. The names come on inside 8 light years of Sol.
take_demo() {
  fly "$work/demo/0000.png" GALOS_SHOT="$work/demo" "${galaxy_pose[@]}" \
    "${inner_pose[@]}" GALOS_SHOT_NAMES_WITHIN=8 \
    GALOS_SHOT_HOLD=300 GALOS_SHOT_WAIT=2100 GALOS_SHOT_EVERY=18 \
    GALOS_SHOT_WIDTH=960 GALOS_SHOT_HEIGHT=540 GALOS_SHOT_SCALE=1
  # A field of stars under a moving camera is new pixels nearly everywhere
  # every frame, which is what a GIF is worst at: 640 wide, one palette of
  # 64 for the whole run and no dither is what keeps it under 10 MB rather
  # than tens of them. Dither crawls from frame to frame, and each doubling
  # of the palette costs another quarter.
  ffmpeg -v error -y -framerate 10 -i "$work/demo/%04d.png" \
    -vf "scale=640:-1:flags=area,tpad=start_mode=clone:start_duration=1:stop_mode=clone:stop_duration=2,split[a][b];[a]palettegen=max_colors=64[p];[b][p]paletteuse=dither=none" \
    -loop 0 "$out/demo.gif"
  echo "$out/demo.gif"
}

(cd "$root" && cargo build --release -q -p galos_map)
mkdir -p "$out"
for target in "${targets[@]}"; do
  "take_$target"
done
