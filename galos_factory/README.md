# galos_factory

The production sim for the galos factory game — a standalone Bevy crate
with no dependency on the 3D map. See [`DESIGN.md`](../DESIGN.md) for the
game design; this file is just how to run it.

## Running

```sh
# Windowed: the demo colony with the egui panels (needs a display).
cargo run -p galos_factory

# Headless: step N ticks and print the production report. No window, no
# GPU, no database — this is the balancing tool.
cargo run -p galos_factory --no-default-features -- --headless 3000

# Either mode can load a different system instead of the embedded Sol
# fixture.
cargo run -p galos_factory --no-default-features -- --headless 3000 my_system.ron
```

Escape quits the windowed app. Speed controls (pause / 1× / 10× / 60×) are
in the top bar; the sim ticks at a fixed 10 Hz and speed changes how many
ticks run per step, so results never depend on the speed you watched at.

The demo scenario is scripted in `demo_scenario` in `src/main.rs`: a
commander buys an outpost on Mercury, bootstraps construction materials
from Abraham Lincoln's market, builds a mine → smelt → purify → assemble
chain, and runs standing contracts to sell computer components and import
fuel, polymers, and life support.

## Tests

```sh
cargo test -p galos_factory --no-default-features
```

Runs headless with `MinimalPlugins`, driving the `SimTick` schedule
directly — no window, no wall clock, so every run is exact.

## Layout

| Path | What |
|---|---|
| `data/*.ron` | items, recipes, buildings — the whole game balance |
| `data/fixtures/sol.ron` | a hand-authored system snapshot |
| `src/data.rs` | loads and validates the RON, interns item ids |
| `src/snapshot.rs` | the sim's view of the outside world (`elite_journal` types) |
| `src/seed.rs` | snapshot → entities: system, factions, bodies, stations |
| `src/sim/` | components and one file per tick stage |
| `src/ui/` | egui panels, shared with the full game |

## Notes for hacking on it

- **Determinism is a hard rule.** Integer/milli state (no floats), one
  seeded `SimRng`, explicit `.chain()` wherever two systems touch the same
  component, and dense `Storage` so iteration order can't vary. If you add
  a system that mutates shared state, order it explicitly.
- **Nothing mutates the sim directly.** UI and scripts send
  `PlayerCommand` events with an issuing actor; `apply_commands` validates
  ownership at the start of each tick.
- **Balance lives in the RON files.** Changing a recipe is a text edit;
  `--headless` shows the effect.
