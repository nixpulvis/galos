# Galos Map
![Galos Starmap Demo](./demo.gif)
![Galos Galaxy Zoom](./galaxy.png)

The map is pointed at one directory, and reads no database of its own.
`--index DIR` is the index `galos-sync --index DIR` bakes, and `GALOS_INDEX`
says the same thing for a machine that always draws the same one; with
neither, the map reads `.galos_index` under wherever it was run from. A
commander's own journal reaches the map the same way everything else does:
`galos-sync --from journal=DIR --index DIR` writes it into the index the map
reads.

```sh
cargo run --release
# The index somewhere other than beside the working directory.
cargo run --release -- --index "$HOME/.galos_index"
GALOS_INDEX="$HOME/.galos_index" cargo run --release
```

## How it draws

The **far field** is everything outside the system the camera stands in: one
spatial hierarchy over every system and everything reading it — the map's
level of detail, the night sky's discrete stars, and the glow behind both. It
is built and read by [`galos_index`](../galos_index), which is also the
client's on-disk format, and that is what lets the map draw the galaxy
without a database.

The **near field** — a system's own stars and planets at real geometry, and
reaching them — is the code under
[`src/systems/bodies`](./src/systems/bodies).

What it draws is a moment rather than a pile of scans. A system's rows arrive
from as many commanders as have ever flown there, so each orbit carries how
long before that moment it was read and is run on from its own reading. So a
planet is drawn where the game has carried it by now rather than where
somebody once saw it.

The moment is the galaxy's, not a system's: one now, the same on both sides
of a flight, and each system answers it from its own newest scan. It is read
in a strip of its own, at the top of the viewport and in the middle of it, in
the game's own calendar — which runs 1286 years ahead of ours and otherwise
with it: `16 DEC 3300 13:45:00`, whether or not the camera is inside a system.

Clicking that reading opens a slider under it, and dragging one runs the map
on past the present; the span it stands at is named beside the date, with a
`Now` to let go of it. The slider covers one turn of whatever it is geared
to, and `Body` and `System` beside the reading are which: one orbit of the
body picked out, laid evenly, or one turn of the widest orbit the system has,
laid by decades — a system's slowest body takes a median 993 times as long to
come round as its fastest, so there is no one span that suits a whole system,
and which of the two is wanted is not something the map can work out from
what was clicked. The switch stands only where both turns are on record. Its
places are marked underneath in the spans a reader thinks in, `now` at the
near end and how long the turn runs at the far one, since a logarithmic rail
says nothing for itself about where along it a day is. The phase slider under
a body's panel is the same control geared to that body alone, and the camera
keeps a body picked out under itself while the moment moves.

The strip is turned off in the settings pane, under General with the names
and the grid. Hiding it puts the map back to the present: the reading is the
only place a run-on is shown and the only way back from one.

The two meet in two places only: the sizing law's context scalar in
[`src/systems/scale.rs`](./src/systems/scale.rs), and the photometric scale
the local star is lit by.

Everything reaches the screen flat. A single float resolves one part in
sixteen million of whatever it holds, and a star sits `1e17` metres out, so a
mark or a name built as a mesh where its system actually is tears apart in
the `f32` clip transform. So every mark and every note — the star field, the
names and their leaders, the rings around what is pointed at and picked out,
and the ruled plane's readouts — is projected to a pixel on the processor in
`f64` and painted flat with egui. The star field is
[`src/systems/field.rs`](./src/systems/field.rs), the names and leaders
[`src/systems/labels.rs`](./src/systems/labels.rs), and the cameras and the
order they draw in [`src/camera.rs`](./src/camera.rs).

## Mouse

| Gesture | What it does |
|---|---|
| Left drag | Swing the camera around what it looks at |
| Right drag | Pan the map across the view |
| Wheel | Zoom in and out |
| Click | Pick out the system or body under the pointer |
| Ctrl, command or shift click | Pick one out alongside the rest, or let go of it |
| Click on empty sky | Let go of everything picked out, and put away what the chrome has open |
| Double click | Fly to what was clicked |
| Click the date at the top | Show or hide the slider that sets the moment |

The bar's rows read the same way as the sky they name. A double click on the
row for a selected system flies to it, as a double click on its star does, and
a double click on a filter's row frames everything that filter admits. A single
click on a row says which of several is the one being worked with, and asks
nothing on a row about something already picked out.

A drag belongs to whatever the press landed on for as long as it lasts, so one
started on a slider goes on talking to the slider wherever the pointer wanders.

The bar's form and the clock's scrubber are put away the same way, being the
same kind of thing: whatever opened them again, a click on empty sky, and the
escape key. The last two mean all of them — an escape over an open form and an
open rail takes both, rather than asking to be pressed twice.

## Keys

| Key | What it does |
|---|---|
| `W` `A` `S` `D` | Pan along the ruled plane, rather than across the view |
| `Q` `E` | Pan down and up through it |
| `Z` `X` | Swing the camera left and right around what it looks at |
| `C` `V` | Lower and raise it over the plane |
| `F` `R` | Zoom in and out |
| `Space` | Fly to what is picked out, one at a time |
| `H` | Go home: Sol, from where the map opened |
| `L` | Show or hide the labels |
| `O` | Show or hide the orbit lines |
| `G` | Show or hide the grid |
| `/` or `Shift-S` | Search the box for a system |
| `Shift-F` | Ask the box for a faction to filter on |
| `Shift-R` | Ask the box for systems to route between |
| `Esc` | Put away the bindings, or everything the chrome has open |
| `F1` or `?` | Show or hide these bindings |
| `F3` | Show or hide the diagnostics window |

Panning and zooming cover ground in proportion to how far out the camera is, so
a key moves the map at about the same rate whether it is looking at the whole
galaxy or at one planet.

Every binding is a key struck on its own, but for the four that want shift: the
three that put a question in the bar's box, and the `?` that opens the key
list. Held with control, command or alt, a key is left alone. So is every one
of them while a field is being typed into, apart from the escape that puts the
field away.

An escape puts away whatever the chrome has open, all of it at once. The
bindings window is the exception and goes first, being read over everything —
a form left standing while something is looked up included.

The map is quit by closing its window.

## Profiling

`--features tracy` builds the map to be profiled and
[Tracy](https://github.com/wolfpld/tracy) reads it: bevy's own zones — one per
system, per schedule and per render pass, and a frame mark per present — plus
the map's, which are the work that happens off the main thread and would
otherwise be unexplained gaps on the pool threads: `index read` (opening),
`refresh poll`, `cell payload` (one per cell a view change asks for, named
with it), `region cells` (one per worker of the legacy region fetch, named
with its share), `route search`, `name search`, `stop lookup` and `bodies
read`. Those are compiled into every build and go wherever the subscriber
sends them, which without the feature is nowhere.

The protocol is versioned and checked when the profiler connects, so the
profiler has to be the release the client speaks: **Tracy 0.13.1**, which is
the `tracy-client-sys` 0.28 in `Cargo.lock`. `brew install tracy`, or a build
of that tag.

```sh
cargo run --release --features tracy -- --index "$HOME/.galos_index"
# In another shell: the profiler to watch it live, or a capture to read after.
tracy
tracy-capture -o galos.tracy -s 20
# Every allocation as well, attributed to the zone that made it.
cargo run --release --features tracy_memory
```

The map first, then the profiler. Until something connects the client holds
everything it is told, which is what bevy warns about on startup — memory
grows until it is read — and a profiler already listening when the map starts
can lose the handshake and exit rather than wait through it.

`RUST_LOG` replaces bevy's filter whole, and a filter that drops a span drops
it from the capture as well as from the log, so a run with `RUST_LOG=warn`
profiles nothing. Leave it unset, or keep `info` in whatever it says.

A release build is the one to read. The dev profile is `opt-level = 1` with
its dependencies at 3, which is playable but is not what the numbers mean.
