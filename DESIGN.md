# Galos Factory — Game Design & Implementation Plan

## Context

Galos maintains a live mirror of the Elite Dangerous galaxy: `galos-sync eddn`
subscribes to EDDN and keeps Postgres in sync with systems, bodies, stars,
stations, factions (influence/states/happiness/conflicts, with an
influence-history trigger table), and station market listings.

The goal is a factory/sim game built on top of this — inspired by Dyson Sphere
Program, but abstract/numeric for v1 — where the live BGS data seeds resources,
markets, and events. Items are real ED commodities; we author the production
chains between them. The player "builds out the world behind the BGS."

Foundational decisions:

- Abstract/numeric sim first (no spatial planet-surface building); the 3D
  galaxy map (`galos_map`) is the main view.
- New crates, reusing `galos_map` (Bevy 0.14.2 + bevy_egui) as a library.
- Items = real EDDN commodity ids, so live market data joins for free.

## A. Game design

### World model

Four nested layers, each contributing one thing:

- **`System`** — the *weather*: faction states, security, population,
  conflicts. Modifies everything below (prices, piracy, productivity, taxes);
  never built on directly.
- **`Body`** — the *geology*: deposits from `planet_class`/`volcanism`,
  gravity, temperature; for stars, class/luminosity. Never built on directly;
  the context a station inherits.
- **`Station`** — the *container and the grid*. All factories are inside a
  station. Placement is **surface** (on a landable body — inherits deposits,
  gravity launch costs, temperature power overhead, geothermal access) or
  **orbital** (inherits star context — full solar, fuel scooping at scoopable
  stars, cheap zero-g logistics). Every station owns a **power grid**, a
  **shared storage pool**, and **factory slots**.
- **`Factory`** — occupies a slot, runs one recipe, draws power from the
  station grid, pulls inputs from station storage, pushes outputs to station
  storage.

**No intra-station logistics**: the station's shared storage is the implicit
belt network — co-located factories work through the common pool.

**All trade between stations is contracts fulfilled by ships.** Just as trade
routes in E:D are contracts, the trade primitive here is a **Contract**:
*(from station, to station, item, quantity or standing rate, pay per unit,
deadline)*. Contracts go on a per-system **contract board**; cargo ships
fulfill them. Both sides are playable:

- **Player-issued**: move goods between your own stations by issuing a
  standing contract and assigning your own ships (pay = 0, you cover fuel) —
  this is what a "supply route" is — or post it publicly with a fee and let
  NPC haulers carry it for you (outsourced logistics: no fleet to manage,
  but you pay per unit and inherit their schedule and risk).
- **NPC-issued**: stations generate contracts from their market state — low
  input stock → import contract at a premium, surplus → export contract.
  Player ships can accept them for credits, which makes **contract hauling
  the natural early game**: buy one ship, work the board, earn starting
  capital before you own a single factory slot.

Ship mechanics underneath: each ship has cargo capacity and fuel per leg, in
ED hauler classes bought at `Shipyard`-service stations (Hauler → Type-6 →
Type-7 → Type-9: capacity vs fuel-burn tradeoffs). Travel time comes from
real distances; throughput of a standing contract is emergent — capacity ×
ships ÷ round-trip time. Every departure burns `hydrogenfuel` from the
origin station's pool (or bought at its market); no fuel, no departure — the
contract stalls, a first-class bottleneck alongside starved/blocked/
brownout. Piracy rolls per ship arrival: a failed roll loses the ship and
its cargo, not an abstract percentage. The DSP puzzle splits cleanly: inside
a station, balance recipe ratios and power; between stations, balance
contracts, fleet size, fuel supply, and risk.

**Where stations come from** — two sources:

1. **Lease factory slots in real NPC stations** (from the DB). Slot count and
   allowed factory kinds derive from `StationType` + `economies` (a large
   Industrial Coriolis offers many assembler slots; an Extraction outpost
   offers refinery slots). Rent goes to the controlling faction, modified by
   its states and influence. Selling at that station's own market is local
   and instant.
2. **Construct player outposts** (surface or orbital) where geology demands
   it — most good deposit bodies have no NPC port. Small (few slots),
   upgradeable, costs goods + credits, no market of their own.

### Game phases

The progression arc doubles as the tutorial — each phase teaches the systems
the next phase depends on:

- **Tutorial — the Hauler.** You start with a single Hauler, like E:D. You
  run contracts off the board to build credits. You build nothing — but
  you've now touched every core system: the board (reading the economy),
  ships and fuel, markets and price curves, piracy and security. The game's
  verbs are learned before its responsibilities.
- **Early game — first outpost.** With enough credits — or a loan — you buy
  an outpost (turnkey, credits-priced; surface or orbital placement chosen
  against real geology) and/or lease NPC slots, and stand up your first
  production chain feeding a nearby market.
- **Mid game — your own network.** Multiple stations, and you start issuing
  your own contracts between them: standing supply contracts flown by your
  fleet or posted publicly for NPC haulers. The gameplay shifts from single-
  station ratios to network design under fuel, risk, and upkeep constraints.
- **Late game — completing the trees, building stations.** Full production
  trees running; construction of true stations from bills of materials
  (multi-tier goods, ED colonization-style manifests), each completed
  station adding slots, markets, and NPC activity — transforming systems.
- **Endgame — politics.** BGS play across the systems you hold interest in:
  faction standing, election and war intervention riding real E:D outcomes,
  political control unlocking perks and construction permits (see
  *Politics: the real endgame*).

### Station upkeep and the local-supply rule

Stations are not free to hold: every station consumes **core upkeep
products** per tick (scaled by size/active slots) from its own storage —
life-support and maintenance items from the regular tree (e.g. `water`,
`hydrogenfuel`, `polymers`, `semiconductors` at higher tiers). Unmet upkeep
degrades the station: slots go offline, then the station shuts down (NPC
stations under unmet upkeep visibly wither — markets thin out, contracts
dry up).

**Galactic security requires local supply**: imports of core upkeep products
for production/upkeep are capped — a station cannot run on upkeep hauled in
from another system; the bulk must originate in-system (NPC market purchases
count as local; inter-system freight does not). Consequences:

- You cannot expand by pure logistics. Every new system must bootstrap a
  local upkeep chain before heavy industry — expansion is industrial, not
  just a longer haul route.
- NPC stations' upkeep is their standing baseline demand on the contract
  board, giving every system a permanent economic floor.
- In the single-system vertical slice the rule is latent (everything is
  local); it becomes the defining constraint of the multi-system era.

### Politics: the real endgame

Like the BGS in E:D itself, the true endgame is elections and war across the
systems you hold interest in. The synced BGS is the political game board —
the player never overwrites real BGS data; political play is a **standing
layer over the live baseline** (the same pattern as player market drawdown
over refreshed listings):

- **Interest.** Per-faction standing, earned by feeding a faction's economy:
  fulfilling its stations' contracts, keeping their upkeep supplied, funding
  election campaigns, supplying war materiel to its side during real
  conflicts.
- **Riding real outcomes.** Elections and wars in synced systems resolve by
  what actually happens in E:D, arriving via EDDN. A pending election
  (`system_faction_states`) opens a campaign-goods demand window; an active
  war (`conflicts`) is a military-supply demand spike for your chosen side.
  Back the winner → standing surges; back the loser → the investment is
  burned. The influence history table (`system_faction_influences`) is the
  form guide — trend lines to bet with.
- **Control.** When a faction you hold top-tier standing with controls the
  system (max influence — real data), you effectively control its politics,
  unlocking: **production perks** (activity rate bonuses, extra lease slots,
  cheaper rents), **cheaper security** (piracy floor cut, convoy insurance),
  **tax breaks**, and **construction permits** — building your own stations
  requires a permit from the controlling faction, binding the political and
  industrial endgames together. Lose the election, lose the perks: a
  controlling-faction flip in a system full of your assets is the late-game
  crisis (and opportunity, if you saw it coming in the trends and switched
  sides early).
- **Portfolio politics.** The multi-system late game is a map of systems
  where you hold interest: timing expansions around election cycles, feeding
  different factions in different systems, deciding where to fight the
  political weather and where to profit from it.

Fantasy framing: you are the industry behind the BGS — the game presents
real outcomes as consequences of your supply lines ("your munitions won the
war for the Sol Workers' Party").

### Influence authority: designed to scale

Today we control nothing about E:D's outcomes — but the design should scale
if we ever do. All political cause-and-effect flows through one interface:
the sim emits **political intents** ("support faction X in system Y with
weight W, before this election resolves"), an **`InfluenceAuthority`**
backend decides what they amount to, and outcomes are *always* read back
from the synced DB — the single source of truth in every tier. Because the
sim never assumes its intents move the world, strengthening the authority
never changes the sim; it only strengthens the causal link.

Three tiers, weakest to strongest:

1. **Mirror (now).** Intents accrue local standing only; outcomes are
   whatever the real BGS does. The player bets and rides.
2. **Closed loop through a commander (near-term, no permission needed).**
   The player (or their squadron) also plays E:D. The game converts intents
   into concrete BGS objectives — "run missions for the Sol Workers' Party",
   "sell at these stations this week", "fight for side A in the war" — the
   commander flies them in E:D, and the sync observes the results. Crucially,
   `galos-sync journal` already ingests the player's own local journal files,
   so their personal actions are **attributable** — the game can verify and
   credit *your* mission runs, trade, and combat bonds (requires extending
   `elite_journal` to parse mission/voucher/trade events, currently limited
   to Location/FSDJump). This is the BGS-player spreadsheet workflow, turned
   into a game: the sim plans, the human flies, EDDN/journal verifies. The
   game's standing and perks respond to *real influence you actually moved*.
3. **Write authority (partnership).** If a partnership with Frontier (or a
   sanctioned shard/hybrid mode) ever grants real write influence, the same
   intents commit through an authority that actually moves the needle. The
   interface doesn't change; the world does.

The vertical slice implements tier 1 and keeps the intent/authority seam in
place; tier 2 is the first post-slice political milestone.

### A living economy

- **NPC stations have factories too — where it makes sense.** Seeded by
  station economy: Industrial stations run assemblers (consume metals,
  produce components into their market stock), Refinery stations run
  refineries (consume ores), Extraction surface ports run extractors,
  HighTech runs top-tier assembly; Tourism/service stations run none. NPC
  factories are the *same* Factory entities and tick through the same
  systems, just faction-owned — they pay no rent and their I/O is their own
  market stock. This makes markets genuinely dynamic: an Industrial station
  actually consumes ore (drawing stock down, raising ore prices → your mining
  is wanted) and actually produces goods (raising stock, capping goods
  prices). Player opportunity = reading and feeding these real flows.
- **NPC trade runs on the same contract board.** NPC stations issue import/
  export contracts from their market state (low input stock → import at a
  premium; surplus → export), and NPC hauler fleets accept and fly them with
  the same ship mechanics, faction-owned. The board is therefore a live
  window into the system's economy: what's scarce, what's glutted, what's
  paying. Player and NPC haulers compete for the same contracts; wars and
  piracy hit NPC ships too — a blockaded Industrial station visibly starves.
  v1 keeps NPC issuance/acceptance on simple heuristics.
- **Economy-type activity modifiers.** The system/station economy grants
  bonuses and penalties by activity class, while geology stays the hard gate:
  Extraction economy → extractor rate bonus (but extraction works anywhere it
  makes sense geologically); Refinery → refining bonus; Industrial →
  assembly bonus; HighTech → tier-3+ bonus; mismatched activities take a mild
  penalty. Layered on top of BGS state modifiers.

### Failure: debt and bankruptcy

Credits go negative into **debt with interest per tick**. Standing
obligations — slot rent, loan repayments on outpost construction, link
upkeep — keep running whether or not production flows, and there are many
ways underwater: overbuilding on borrowed credits, crashing your own prices
by oversupplying a market, piracy losses on unescorted links, a state flip
(Bust/Lockdown) gutting your revenue. Sustained insolvency past a debt
ceiling = **bankruptcy, the complete-failure state**. The tension between
expansion (borrow to build) and resilience (buffer against BGS weather) is
the core economic risk game.

### Core loop (vertical slice, target system: Sol)

1. Browse the existing 3D galaxy map, select a populated system.
2. System view (egui): bodies with derived deposits, stations with market
   summaries and leasable slots.
3. Lease slots in an NPC station and/or construct an outpost; install
   factories from a build menu: Extractor, Fuel Scoop, Refinery, Assembler,
   Power Plant, Solar Array, Geothermal Plant. Each factory = a row with a
   recipe selector; extractors surface-only, scoops orbital-only.
4. Work the contract board: accept NPC hauling contracts for income, issue
   standing contracts between your own stations (assigning your ships), or
   post public contracts for NPC haulers to carry. Travel time from real
   orbital distances, fuel burned per leg, piracy risk from
   security/conflicts.
5. Sell to NPC station markets seeded from real `listings` (price/demand
   drawdown + regen) — instantly where you lease, via haul links elsewhere.
6. Watch rate/bottleneck dashboards (starved inputs, power brownouts) — the
   DSP ratio-fixing loop.
7. React to live BGS: states shift prices/demand and rents, conflicts raise
   piracy, EDDN updates drift NPC prices.

### BGS → gameplay seeding (all real columns)

| Source | Mapping |
|---|---|
| `bodies.landable` | gates surface facilities |
| `bodies.planet_class` | deposit table: Metal rich → bauxite/rutile/gallite/coltan (high richness); HMC → medium; Rocky → bauxite/cobalt low, + mineraloil if atmosphere; Icy/Rocky ice → water |
| `bodies.volcanism` | +richness on metals; unlocks Geothermal Plant |
| `bodies.surface_gravity` | build cost + link launch-cost multiplier |
| `bodies.surface_temperature`, `atmosphere_type` | facility power overhead |
| `bodies.semi_major_axis`, `stations.dist_from_star_ls` | ship route travel time (ticks per leg) |
| `stations.services` (contains Shipyard) | where cargo ships can be bought |
| `stars.star_class`/`luminosity` (fallback `systems.primary_star_class`) | Solar Array output; scoopable classes unlock Fuel Scoop Platform (hydrogenfuel) |
| `systems.population` | labor pool → facility count cap (log10 tiers) + NPC demand floor |
| `stations.ty` (StationType) + `stations.landing_pads` | leasable factory slot count (large Coriolis/Orbis ≫ Outpost) and whether the station is surface or orbital placement |
| `systems.primary/secondary_economy`, `stations.economies` (EconomyShare[]) | which commodities each station buys/sells and at what weight (Industrial buys metals, HighTech pays premium for T3, etc.); also which factory kinds its leased slots allow |
| `listings` (mean/sell price, demand, brackets, stock) | seeds each market's price curve: `base_price` from mean_price, `stock` and `demand_baseline` from stock/demand, brackets set NPC consumption rate; synthesize from item base_price when no listing |
| `system_factions.state` + `system_faction_states` | price/demand modifiers (Boom +25% sell/+50% regen; Bust −20%; Investment −25% build cost; InfrastructureFailure → powergenerators demand ×3; Lockdown/CivilUnrest throughput −50%; …). Pending states shown as forecasts in the UI |
| `system_factions.influence` (max = controlling) | tax rate on sales; fragmented influence → higher tax/volatility; Anarchy → no tax but piracy floor |
| `system_factions.happiness` | labor productivity (craft speed ±) |
| `systems.security` | piracy base rate on links (High 0% → anarchy 10%) |
| `conflicts` | timed events: piracy + military-goods demand spikes |
| `system_faction_influences` (history) | later: trend lines / instability forecasts |

### Items, recipes, buildings

- **RON data files** in `galos_factory/data/{items,recipes,buildings}.ron`,
  embedded via `include_str!`. The item set is a superset of E:D's: ED items
  keep their EDDN internal ids (joining `listings.name` directly for live
  prices); galos-unique items (production grades, invented intermediates)
  carry our own ids with `ed: false` and price purely from `base_price`.
  Fields: display name, category, tier, base_price, ed flag.
- Recipes: building type, inputs/outputs `[(item, qty)]`, ticks, power.
  Buildings: cost, power, site kind (SurfaceLandable | Orbital |
  StarProximity).
- **Starter tree (~20 real commodities, 4 tiers):**
  - T0 raw: bauxite, rutile, gallite, coltan, cobalt, water, mineraloil,
    hydrogenfuel
  - T1 refined: aluminium, titanium, gallium, tantalum, polymers
    (+ water→hydrogenfuel fallback recipe)
  - T2 components: semiconductors (gallium+polymers), superconductors
    (titanium+cobalt), computercomponents (semiconductors+aluminium)
  - T3 goods: powergenerators, robotics, consumertechnology, autofabricators
  - ⚠ Verify every id against `SELECT DISTINCT name FROM listings` (e.g. it's
    `robotics`, not "roboticcomponents"); a seed-time/CI check should flag ids
    that never appear in live listings.

### Resource catalog (T0/T1)

All real EDDN commodity ids (verified against `listings.name` in M2),
organized by source — geology is the extraction gate:

**T0 ores** (surface mining, metal-rich/HMC bodies), each with one refinery
pair: `bauxite`→`aluminium`, `rutile`→`titanium`, `gallite`→`gallium`,
`coltan`→`tantalum`, `bertrandite`→`beryllium`, `indite`→`indium`,
`lepidolite`→`lithium`, `uraninite`→`uranium`; plus `cobalt` (used direct).

**T0 native metals**: `copper`, `silver`, `gold`, `platinum`, `palladium`,
`osmium` — mined as **raw grade** (see *Grades* below), purified to
production grade at a Refinery. In E:D these are ring-mined; ring data is
not yet synced (journal `Scan` Rings array is dropped) — v1 sources them
from metal-rich surface deposits; ring-mining platforms arrive when the
sync gains a rings table.

**Grades — nothing straight from mining is production grade.** The item set
is a **superset of E:D's commodities**: ED items are the *as-found* world
goods the real galaxy trades — so ED's `copper` IS raw mined copper, and it
buys/sells at live listing prices exactly as-is — while **production grades
are unique galos items** (`Pure Copper`, etc.) filling in what ED doesn't
model. Two refinery process families: **Smelt** (ore → metal: `bauxite` →
`aluminium`; ED's ores are the raw form, ED's smelted metals count as
production grade) and **Purify** (ED native metal → pure galos grade:
`copper` → `Pure Copper`; always **1:1** — purification costs only time and
power, never material). Higher recipes generally require production grade;
later, alloy/composite recipes (`ceramiccomposites`, `cmmcomposite`,
`insulatingmembrane`) may accept raw ED metal directly — dirtier, cheaper
paths that skip purification. Galos-unique items have no live listing:
their prices synthesize from `base_price`, and they appear in NPC markets
only where NPC factories use them — otherwise they live in player chains
and contracts.

**T0 icy**: `water`, `lowtemperaturediamond` (sale valuable, no chain),
`methaneclathrate`, `tritium` (late-game fuel branch).

**T0 liquids/gases**: `mineraloil` (rocky + atmosphere), `hydrogenfuel`
(star scoop; electrolysis fallback), `liquidoxygen` (electrolysis
byproduct).

**T0 agriculture** (Earth-likes, water worlds, Agri stations): `grain`,
`fruitandvegetables`, `animalmeat`, `fish`, `coffee`, `tea`, `algae`.

**T0 population byproduct**: `biowaste` — emitted by populated stations,
consumed by agriculture as fertilizer (closed loop).

**T1 processing**: the eight refined metals; `polymers` ← mineraloil;
`hydrogenfuel` ← water (bad ratio, `liquidoxygen` byproduct); `explosives`
← methaneclathrate/mineraloil; `syntheticfabrics` ← polymers (agri-side:
`naturalfabrics`, `leather`); `syntheticmeat` + `foodcartridges` ←
algae/grain (cheap mass upkeep food vs premium real food); `basicmedicines`
← agri + chemicals (upkeep for high-population stations).

Upper tiers sit on the metals+polymers spine (semiconductors,
superconductors, computercomponents → powergenerators, robotics,
consumertechnology, autofabricators); foods/textiles/medicines/explosives
branches feed upkeep, war supply, and population growth.

### MVP recipe table (prototype targets)

~25 items, 4 tiers; numbers are first-pass targets for headless balancing
runs, tuned so ratios don't divide cleanly (that's where the gameplay is).
Pure grades (galos-unique) sit in mainline chains so Purify is mandatory.

**Extraction** (output × deposit richness; Extractor surface, Scoop orbital):

| Recipe | Building | Site | Output | Ticks | MW |
|---|---|---|---|---|---|
| Mine Bauxite | Extractor | metal-rich/HMC | 2 bauxite | 4 | 4 |
| Mine Rutile | Extractor | metal-rich/HMC | 2 rutile | 4 | 4 |
| Mine Gallite | Extractor | metal-rich | 1 gallite | 4 | 4 |
| Mine Cobalt | Extractor | metal-rich/HMC | 1 cobalt | 4 | 4 |
| Mine Copper | Extractor | metal-rich/HMC | 2 copper | 4 | 4 |
| Ice-mine Water | Extractor | icy body | 4 water | 4 | 3 |
| Pump Mineral Oil | Extractor | rocky + atmosphere | 2 mineraloil | 4 | 3 |
| Harvest Algae | Extractor | water world / ELW | 2 algae | 5 | 2 |
| Scoop Hydrogen | Fuel Scoop | orbital, scoopable star | 3 hydrogenfuel | 5 | 2 |

**Refinery** (Smelt / Purify / Chemistry — Purify cheaper than Smelt;
electrolysis is the power-hungry no-scoopable-star fallback):

| Recipe | Inputs | Outputs | Ticks | MW |
|---|---|---|---|---|
| Smelt Aluminium | 3 bauxite | 2 aluminium | 6 | 6 |
| Smelt Titanium | 3 rutile | 1 titanium | 8 | 8 |
| Smelt Gallium | 2 gallite | 1 gallium | 6 | 6 |
| Purify Copper | 1 copper | 1 Pure Copper | 3 | 4 |
| Purify Cobalt | 1 cobalt | 1 Pure Cobalt | 3 | 4 |
| Crack Polymers | 2 mineraloil | 3 polymers | 6 | 5 |
| Electrolyse Water | 4 water | 2 hydrogenfuel + 1 liquidoxygen | 8 | 10 |

**Assembler**:

| Recipe | Inputs | Outputs | Ticks | MW |
|---|---|---|---|---|
| Semiconductors | 1 gallium + 2 polymers | 2 semiconductors | 8 | 8 |
| Superconductors | 1 titanium + 2 Pure Cobalt | 1 superconductors | 10 | 12 |
| Computer Components | 2 semiconductors + 1 aluminium + 1 Pure Copper | 2 computercomponents | 10 | 10 |
| Food Cartridges | 2 algae + 1 water | 3 foodcartridges | 6 | 4 |
| Power Generators | 1 superconductors + 2 titanium + 1 Pure Copper | 1 powergenerators | 15 | 12 |
| Consumer Technology | 2 computercomponents + 2 polymers | 1 consumertechnology | 12 | 10 |
| Robotics | 2 computercomponents + 1 superconductors + 1 aluminium | 1 robotics | 15 | 12 |
| Autofabricators (capstone) | 1 robotics + 2 computercomponents + 1 titanium | 1 autofabricators | 20 | 15 |

**Power**:

| Source | Consumes | Produces | Notes |
|---|---|---|---|
| Power Plant | 1 hydrogenfuel / 10 ticks | +20 MW | anywhere |
| Solar Array | — | +8 MW × star mult | orbital full, surface ×0.5 |
| Geothermal Plant | — | +15 MW | surface + volcanism |

**Construction costs & building upkeep** — every factory and power plant
costs items to build and consumes maintenance items while it stands
(per 100 ticks). Construction inputs are deliberately buyable ED goods
(bootstrap: buy materials at NPC markets before you can produce them);
raw `copper` is usable directly in cruder construction:

| Building | Construction cost | Upkeep / 100t |
|---|---|---|
| Extractor | 20 aluminium + 4 polymers + 1 powergenerators | 1 polymers |
| Fuel Scoop | 15 aluminium + 6 polymers + 1 powergenerators | 1 polymers |
| Refinery | 30 aluminium + 10 titanium + 2 powergenerators | 2 polymers |
| Assembler | 25 aluminium + 4 semiconductors + 1 powergenerators | 2 polymers |
| Power Plant | 15 titanium + 10 aluminium + 8 copper | 1 polymers |
| Solar Array | 12 aluminium + 4 semiconductors + 6 copper | 1 polymers / 200t |
| Geothermal Plant | 20 titanium + 10 copper | 2 polymers |
| Storage Module | 15 aluminium + 2 polymers | 1 polymers / 200t |

**Station upkeep** (life support, on top of building maintenance):
1 water + 1 foodcartridges per active slot / 100 ticks — MVP set; higher
tiers add medicines etc. Unpaid building upkeep degrades that building
(slowdown → offline); unmet station upkeep degrades the whole station.

`hydrogenfuel` is deliberately contested three ways: power plants, ship
legs, and the water-for-fuel-vs-upkeep electrolysis question. `polymers`
is the universal maintenance drain — every standing structure bleeds it.

### Production model

- Fixed discrete tick (1 tick = 1 in-game second; engine stepped from Bevy
  `FixedUpdate` at 10 Hz; pause/1x/10x/60x). `tick()` is pure & deterministic:
  `(GameState, &StaticData, &SystemContext) -> Vec<SimEvent>`.
- Facilities: recipe, input/output buffers with capacities, craft progress,
  power draw. Per-site power balance first; deficit → proportional slowdown
  (DSP brownout), no hard stop.
- Extractors: rate = base × richness × happiness; deposits infinite in v1.
- Trade: ship cargo routes. Each ship is a small state machine
  (Loading → Outbound → Unloading → Return), consuming fuel per leg from the
  origin station; throughput emerges from cargo capacity × fleet size ÷
  round-trip ticks. Piracy roll per arriving ship — lose the ship + cargo.
  `hydrogenfuel` thus feeds both power plants and logistics, making the fuel
  chain (scooping, water electrolysis fallback) strategically central.
  Inter-system routes (multi-system era) reuse the FSD jump-range/fuel-cost
  modelling already in `src/bin/galos/route.rs` — jumps cost real fuel by
  ship mass and FSD class.
- Markets: per-station per-item **supply/demand price curve**, ED-style —
  price is a function of current stock vs demand baseline (seeded from
  `listings`), so surplus supply depresses prices and scarcity raises them.
  Selling into a market raises its stock and pushes the price down under you;
  NPC factory consumption draws stock down and prices recover. BGS state
  modifiers shift the curve, not a static price. Sell/Buy orders convert
  goods ↔ credits along the curve.
- Storage pressure is real: a full station pool stalls factories ("output
  blocked" — the third status besides starved and brownout). Storage is
  expanded by dedicating factory slots to storage modules.

### Save model

Same Postgres DB, **dedicated `factory` schema** owned by a new
`galos_factory_db` crate with its own migrations dir (`galos_db/migrations/`
untouched; both migrators run with `Migrator::set_ignore_missing(true)`,
factory migration timestamps start after 2024-09-20). Tables: `factory.saves`
(id, name, system_address, credits, sim_tick, timestamps) + child tables
(facilities, links, inventories, market_state) FK'd to save id. Autosave
upsert every N sim-seconds. Multiplayer shares one world; how commanders
interact with faction treasuries is deliberately unmodelled for now. `GameState` is serde-serializable → RON
export doubles as test fixture format.

Rationale: the sim continuously joins live BGS tables, and the map already
requires the DB; a local save file would force snapshotting all of it.

## B. Architecture

The game is built from two core parts:

- **A) The production system** — a standalone Bevy crate (`galos_factory`)
  with no dependency on the 3D mapping code. It runs independently and its
  egui panels/stat views are plugins shared with the full game.
- **B) The map** — `galos_map` becomes a shared library. Generic additions
  (e.g. a public system-selection event) land in `galos_map` itself;
  game-specific front-end work lands in the new `galos_game` crate.

### Crates (added to `[workspace] members` in `Cargo.toml`)

```
galos_factory/       # A) standalone Bevy crate: production sim + its UI panels
  src/lib.rs         #   sim_plugin: resources, SimTick schedule, SimSets
  src/data.rs        #   RON items/recipes/buildings, interned + validated
  src/snapshot.rs    #   the sim's view of the world (elite_journal types)
  src/seed.rs        #   snapshot -> system, factions, bodies, stations
  src/sim/components.rs
  src/sim/{commands,control,power,upkeep,extract,craft,shipping,market,stats}.rs
  src/ui/mod.rs      #   bevy_egui panels (ui feature), shared with the game
  src/main.rs        #   standalone runner: windowed, or --headless N
  data/*.ron  data/fixtures/  tests/
galos_factory_db/    # persistence + snapshot loading — galos_db, galos_factory, sqlx
  migrations/  src/{save,snapshot}.rs
galos_game/          # bin crate — galos_map (lib) + galos_factory plugins + glue
  src/{main,colony,system_view,refresh}.rs
```

- **`galos_factory` is Bevy-native but map-free.** Deps: bevy (no render
  features needed by the sim itself), bevy_egui, serde, ron. Sites,
  facilities, and transfer links are entities with components; the sim is an
  ordered chain of `FixedUpdate` systems (`power_balance → extract → craft →
  transfer → market`) at `Time::<Fixed>::from_hz(10.0)`, with a `SimSpeed`
  resource (pause/1x/10x/60x). Player actions are Bevy events applied at tick
  boundaries, keeping the sim deterministic. Its `main.rs` runs the whole
  production game against a fixture or snapshot file — usable for
  development, balancing, and demos without ever opening the map.
- **Panels are plugins.** The egui panels (build menu, facility/link lists,
  rate + bottleneck dashboards, event ticker) live in
  `galos_factory::ui` and are added by both the standalone runner and
  `galos_game`, so every stat view built for the sim is automatically
  available in the full game.
- **Snapshot boundary**: `galos_factory` still never touches the DB. It
  defines snapshot input structs (`BodySnapshot`, `StationSnapshot`,
  `ListingSnapshot`, `FactionSnapshot`, `StarSnapshot`) mirroring the columns
  above; `galos_factory::sim::seed` maps snapshots → deposits, power
  multipliers, and market seeds. Providers fill them: `galos_factory_db` via
  sqlx today, an HTTP client later (see Distribution). Seeding is thus
  testable from RON fixtures with no infrastructure.
- **`galos_game` composes, doesn't implement**: like `galos_map/src/main.rs`
  it is plugin composition — map plugins + `galos_factory::{sim_plugin,
  ui_plugin}` + game glue (system selection → establish colony, snapshot
  fetch, BGS refresh, saves). Additive `galos_map` changes only, e.g. expose
  a public system-selection event from picking in
  `galos_map/src/systems/spawn.rs`.
- All DB IO copies the proven async pattern from
  `galos_map/src/systems/fetch.rs` (AsyncComputeTaskPool + task map polled per
  frame, `Db` resource cloned in). Never copy `search.rs`'s main-thread
  blocking.

### Distribution (decided direction, deferred work)

Shipping Postgres+PostGIS inside a game is a non-starter (PostGIS drags in
platform-specific GEOS/PROJ builds, and players would still need the EDDN
firehose + EDSM seed). The plan:

- **Hosted galaxy mirror**: one server runs Postgres+PostGIS +
  `galos-sync eddn`; `galos_server` grows a small JSON API serving exactly the
  snapshot structs. Self-hosters get docker-compose; dev keeps direct sqlx.
- **Static star catalog**: system positions/names/classes are near-static.
  Bundle (or lazily download) a catalog of populated systems (~20k, a few MB
  compressed) so the map renders with zero spatial queries at runtime; the
  spyglass becomes an in-memory filter. PostGIS stays server-side for sync
  and route-finding (`/route` runs the whole pathfind next to the data).
- **Live data is per-system and tiny**: the system view and 60s BGS refresh
  fetch a few KB of JSON. The map's async fetch architecture already
  tolerates the added latency.
- **Local saves** (RON files or SQLite) for the shipped game; the `factory`
  Postgres schema remains the dev/self-host path.

### Live BGS refresh

`galos-sync eddn` keeps running as a separate process.
`galos_game/src/refresh.rs` re-queries snapshots for the active system every
~60s (same task pattern) → `seed::refresh_context(...)` updates prices/demand
targets/modifiers and emits diff events for a UI ticker ("Boom active in Sol —
electronics +25%"). Geology (deposits/power) is seeded once per save; only
market/faction/conflict data refreshes. Player demand-drawdown is layered over
the refreshed baseline so a refresh never resurrects consumed demand.

### New galos_db work (minimal)

1. `galos_db/src/markets/fetch.rs` (currently commented out in
   `markets/mod.rs`): `Market::fetch_by_system`, `Listing::fetch_by_market`,
   `Listing::fetch_for_system` (join per `galos_db/sql/listings.sql`); fix
   `Listing.updated_at: i64` → `DateTime<Utc>` to match the `listed_at
   timestamp` column.
2. `galos_db/src/stars/fetch.rs` (commented out):
   `Star::fetch_all(db, system_address)`.
3. Existing fetches for bodies/stations/systems/factions suffice.
4. Separate chore (not blocking): add the missing `updated_at` recency guard
   to `System::from_journal` in `galos_db/src/systems/create.rs`.

### Testing

- Deterministic tick tests run a headless `App` (`MinimalPlugins` +
  `sim_plugin`, manual `Time::<Fixed>` stepping — no window, no GPU, CI-safe):
  exact buffer contents after N ticks; 10k-tick conservation-of-mass smoke
  test.
- RON data validation: every recipe io id exists; every non-T0 item
  producible; reachable from T0; no orphans; ids appear in live
  `listings.name` (env-gated).
- Seeding golden tests: captured Sol snapshot fixtures → `SystemContext` vs
  committed golden RON.
- Save round-trip (serde equality) + one env-gated `DATABASE_URL` integration
  test. `cargo test -p galos_factory` runs clean in CI with no DB.

## C. `galos_factory` crate design

### Features & module tree

Feature-gated so the sim compiles without any windowing/render stack:
default = `["ui"]`; the `ui` feature pulls `bevy_egui` (and bevy's render
features); the bare sim needs only bevy's ECS/app/time. Headless tests and
CI build with `--no-default-features`.

```
galos_factory/
  Cargo.toml           # bevy (minimal features) + serde + ron; ui: bevy_egui
  data/
    items.ron  recipes.ron  buildings.ron
    fixtures/sol.ron   # captured SystemSnapshot for standalone play + tests
  src/
    lib.rs             # pub fn sim_plugin(&mut App); pub fn ui_plugin (ui)
    data.rs            # ItemDef/RecipeDef/BuildingDef + RON load + validation
    snapshot.rs        # SystemSnapshot input structs (pure serde)
    seed.rs            # SystemSnapshot -> spawned sites/markets/modifiers
    save.rs            # SaveGame serde round-trip of live sim state
    sim/
      mod.rs           # schedule wiring, SimClock, SimSpeed, SimRng
      components.rs    # Site, Facility, buffers, Link, Market, ...
      commands.rs      # PlayerCommand event + apply_commands
      power.rs  extract.rs  craft.rs  transfer.rs  market.rs
      stats.rs         # rolling rates, bottleneck detection, SimNotice
    ui/
      mod.rs           # ui_plugin, SelectedSite resource
      time.rs  build_menu.rs  facilities.rs  links.rs  stats.rs  ticker.rs
    main.rs            # standalone runner (requires ui)
```

### Static data (`data.rs`)

RON ids are strings (EDDN commodity names); at load they intern to dense
indices for cheap lookup/copy:

```rust
pub struct ItemId(u16);            // index into StaticData.items
pub struct RecipeId(u16);
pub enum BuildingKind { Extractor, FuelScoop, Refinery, Assembler,
                        PowerPlant, SolarArray, Geothermal, Depot }

pub struct ItemDef    { id: String, name: String, category: Category,
                        tier: u8, base_price: u32 }
pub struct RecipeDef  { id: String, building: BuildingKind,
                        inputs: Vec<(ItemId, u32)>, outputs: Vec<(ItemId, u32)>,
                        ticks: u32, power_mw: u32 }
pub struct BuildingDef{ kind: BuildingKind, cost: Vec<(ItemId, u32)>,
                        credits_cost: u32, power_mw: i32,  // negative = producer
                        upkeep: Vec<(ItemId, u32)>,        // per 100 ticks
                        site: SiteKind }

#[derive(Resource)] pub struct StaticData { items, recipes, buildings, by_name }
```

`StaticData::load()` parses the embedded RON and runs the validation pass
(unknown ids, orphan items, unreachable tiers) — the same code the data test
calls.

### ECS model (`sim/components.rs`)

Everything serde-serializable; quantities are integers. Fractional rates use
milli-item accumulators — **no floats in sim state** (floats allowed in
derived stats only), so ticks are exactly reproducible.

Entities mirror the world model: Body entities carry geology, Station
entities are the buildable containers, Factory entities live in stations.

```rust
// Actors: an economic actor holds an account and owns assets. Factions are
// corporations (they run the NPC economy); commanders are players.
#[derive(Component)] struct Faction { name: String }
#[derive(Component)] struct Commander { name: String }
#[derive(Component)] struct Credits(i64);
#[derive(Component)] struct Debt { interest_milli: u32, ceiling: i64 }
#[derive(Component)] struct Ledger { produced/consumed/sold, revenue, expenses }
#[derive(Component)] struct MemberOf(Entity);   // which faction an actor flies for
#[derive(Component)] struct OwnedBy(Entity);    // on stations, ships, contracts

// Star systems: the weather. Environment is intrinsic; Control is derived
// each tick from the Presence entities (one per faction per system,
// mirroring `system_factions`).
#[derive(Component)] struct StarSystem { address: i64, name: String }
#[derive(Component)] struct SystemEnv { piracy_milli, solar_milli, scoopable_star }
#[derive(Component)] struct Presence { faction: Entity, system: Entity,
                                       influence: f32, state: State,
                                       happiness: Happiness }
#[derive(Component)] struct Control { faction: Option<Entity>, tax_milli: u32,
                                      productivity_milli: u32, boom: bool }

// Bodies: geology context only, spawned from the snapshot.
#[derive(Component)] struct Body { name: String, dist_ls: u32 }
#[derive(Component)] struct Deposits(Vec<(ItemId, u32)>);   // richness, milli
#[derive(Component)] struct BodyEnv { volcanism: bool, overhead_milli: u32 }
#[derive(Component)] struct InSystem(Entity);

// Stations: the container + the grid. Factories are their ECS children.
#[derive(Component)] struct Station { name: String, placement: Placement,
                                      dist_ls: u32 }
#[derive(Component)] struct Slots { total: u32 }
#[derive(Component)] struct Storage { /* dense by ItemId */ cap: u32 }
#[derive(Component)] struct PowerGrid { supply_mw, demand_mw,
                                        satisfaction_milli }  // out of 1000
#[derive(Component)] struct LifeSupport { ok: bool }
#[derive(Component)] struct Shipyard;                          // marker

// Factories: one slot, one recipe; pull/push the station's shared Storage.
// Only work-in-progress is held locally (inputs consumed at craft start).
#[derive(Component)] struct Factory { kind: BuildingKind }     // station = Parent
#[derive(Component)] struct ActiveRecipe(RecipeId);            // absent = idle
#[derive(Component)] struct OutputCap(Option<u32>);            // anti-hoarding
#[derive(Component)] struct CraftProgress { progress_milli: u64, holding: bool }
#[derive(Component)] struct MaintenanceDue;                    // marker = offline

// Logistics: contracts fulfilled by ships. A "supply route" is a standing
// self-issued contract with assigned ships; throughput emerges from fleet
// size × cargo cap ÷ round-trip ticks; fuel gates every departure.
#[derive(Component)] struct Contract { from: Entity, to: Entity, item: ItemId,
                                       pay_per_unit: u32,   // 0 for self-contracts
                                       target: Option<u32>, // dest ceiling
                                       reserve: u32 }       // origin floor
#[derive(Component)] struct Ship { class: ShipClass,  // Hauler, Type6/7/9
                                   contract: Option<Entity>, state: ShipState }
enum ShipState { Idle { at: Entity }, Loading,
                 Outbound { ticks_left: u32, cargo: u32 },
                 Returning { ticks_left: u32 } }

// NPC market, on faction-owned stations. Prices derive from the curve each
// tick: price = base × curve(stock / demand_baseline) × modifiers.
#[derive(Component)] struct Market { entries: HashMap<ItemId, MarketEntry> }
struct MarketEntry { base_price: u32,          // seeded from listings.mean_price
                     stock: u32,               // seeded from listings.stock; moved
                                               // by sales + NPC consumption
                     demand_baseline: u32,     // seeded from listings.demand
                     consumption_milli: u32 }
```

Ownership is a reference, not a tag, and every command is validated against
it — an actor may only spend its own money and act on its own assets. This
is what makes multiplayer and the NPC economy the same code path: selling
into a market debits the owning faction's treasury, pays the seller, and
hands the system's controlling faction its tax.

Credits and debt: `Credits(i64)` may go negative; a `Debt { principal,
interest_milli, ceiling }` resource accrues per tick — crossing the ceiling
for a sustained period triggers bankruptcy (run over).

Extractor factories require `Placement::Surface` and mine the host body's
`Deposits`; Fuel Scoops require `Placement::Orbital` with a scoopable
`StarEnv`. Solar Arrays read `StarEnv.solar_mult` (full orbital, penalized on
surface); Geothermal requires surface + volcanism. Rent and market taxes are
charged in `market_tick`.

Resources & events:

```rust
#[derive(Resource)] struct SimClock { tick: u64 }
#[derive(Resource)] enum  SimSpeed { Paused, X1, X10, X60 }  // ticks per fixed step
#[derive(Resource)] struct SimRng(ChaCha8Rng)                // seeded per save
#[derive(Resource)] struct Credits(i64)
#[derive(Resource)] struct SystemModifiers { tax_milli, piracy_milli,
                     productivity_milli, price_mods: Vec<(ItemId, i32)>, .. }
#[derive(Resource)] struct Standing(HashMap<FactionId, i32>)  // political layer;
                     // perks apply when top-tier with the controlling faction

#[derive(Event)] enum PlayerCommand { Build{site, kind}, Demolish(Entity),
                     SetRecipe(Entity, Option<RecipeId>),
                     CreateLink{from, to, item, rate}, RemoveLink(Entity),
                     PlaceOrder{station, order}, SetSpeed(SimSpeed) }
#[derive(Event)] enum SimNotice { Brownout{site}, Starved{facility, item},
                     OutputFull{facility}, Sold{station, item, qty, credits},
                     PiracyLoss{link, qty}, StateChange{..} }
```

### Tick loop (`sim/mod.rs`)

One `FixedUpdate` at a constant 10 Hz; `SimSpeed` runs the tick chain 0/1/10/60
times per step via `run_if` + an exclusive-system loop wrapper — the fixed
clock never changes, so speed does not perturb determinism. Chained order:

```
apply_commands   // drain PlayerCommand, mutate world at tick boundary
power_balance    // per station: supply (plants burn fuel from storage,
                 // solar×StarEnv, geothermal) vs demand; write satisfaction
extract          // rate × richness × productivity × satisfaction
                 // → accum → station Storage
craft            // pull recipe inputs from station Storage at craft start,
                 // progress × satisfaction, push outputs to Storage
upkeep           // stations consume life-support items, buildings consume
                 // maintenance items, from Storage; shortfall streaks →
                 // building slowdown/offline, slots offline → shutdown;
                 // enforce local-supply caps on imports (multi-system era)
contracts        // NPC stations issue/expire contracts from market state
                 // (upkeep needs first); idle NPC (and auto-assigned player)
                 // ships accept from board
shipping         // ships: step state machines; departures load cargo + burn
                 // hydrogenfuel from origin Storage (no fuel → NoFuel stall);
                 // piracy roll (SimRng) per arrival — lose ship + cargo;
                 // contract pay_per_unit settles on delivery
market_tick      // orders vs price curves: stock moves, taxes, rent, Credits
stats            // rolling per-item rates, bottleneck flags, SimNotice out
```

Systems are `.chain()`ed and effectively single-threaded within the tick —
correctness first; parallelism inside a tick is a later optimization.

### Seeding & refresh (`snapshot.rs`, `seed.rs`)

`SystemSnapshot { system, stars, bodies, stations, factions, listings }` —
pure serde structs mirroring galos_db columns. `seed::apply(world, &snapshot)`
spawns sites/markets and computes `SystemModifiers` per the BGS mapping table
(section A). `seed::refresh(world, &snapshot)` updates only market entries and
modifiers (never geology, never player drawdown below the new baseline). The
host decides where snapshots come from: RON fixture (standalone runner),
sqlx (`galos_factory_db`), HTTP (future).

### UI plugins (`ui/`)

All panels read/write the ECS directly; shared `SelectedSite(Option<Entity>)`
+ `SelectedFacility` resources. Panels: time controls (pause/speed, tick
readout), build menu (per selected site, costs + affordability), facilities
table (recipe picker, buffer bars, status chip: Running/Starved/Full/
Brownout), links editor, stats dashboard (per-item production vs consumption
rates, worst-bottleneck list), notice ticker. Panels emit `PlayerCommand`
events only — never mutate sim state directly — so every UI action is
tick-aligned and replayable.

### Standalone runner (`main.rs`)

`DefaultPlugins` + `EguiPlugin` + `sim_plugin` + `ui_plugin`, loads a
`SystemSnapshot` RON from argv (default `data/fixtures/sol.ron`). A
`--headless <ticks>` flag runs `MinimalPlugins`, steps N ticks, prints the
stats table — the balancing tool.

### Testing

- `tests/tick.rs`: `MinimalPlugins` + `sim_plugin`, hand-built world, step N
  fixed updates, assert exact buffer/credit values; 10k-tick conservation
  test (sum of all items + recipe deltas + piracy + sales balances).
- `tests/data.rs`: RON validation pass over the shipped data files.
- `tests/seed.rs`: fixture snapshot → seeded world vs golden expectations.
- `tests/save.rs`: seed + run 100 ticks + save → load → run 100 more ==
  run 200 straight (determinism + round-trip in one test).

## D. Roadmap

- **M0 — Chores (½ day):** `git submodule update --init`; verify build on
  pinned nightly; DB up + `galos-sync eddn` running, Sol has fresh listings.
  No broad dep-pinning (`Cargo.lock` already pins); pin individual crates only
  if a build breaks. *Demo: workspace builds, map runs.*
- **M1 — Standalone production sim:** `galos_factory` crate: RON data
  (20-item tree), body/station/factory/contract/ship entities, the
  FixedUpdate sim chain (power, upkeep, contracts, shipping, price-curve
  markets), first egui panels (facilities, contract board, rates,
  bottlenecks), headless tick + data tests, standalone runner against a
  fixture snapshot. *Demo: `cargo run -p galos_factory` — a bottlenecked
  bauxite→aluminium chain plus a working contract board, no map involved.*
- **M2 — DB plumbing + seeding:** markets/stars fetch APIs;
  `galos_factory::seed` (full mapping table); `galos_factory_db` (snapshots,
  factory-schema migrations, save/load); Sol golden fixtures; commodity-id
  verification against listings. *Demo: headless run seeded from live Sol.*
- **M3 — `galos_game` shell:** bin composing galos_map plugins +
  `galos_factory::{sim_plugin, ui_plugin}`; expose system-selection event
  from map picking (in galos_map, as a generic addition); System view panel
  (bodies+deposits, stations+markets) via async fetch tasks; "Establish
  Colony" creates a save. *Demo: click Sol on the map, read its real
  bodies/markets in-game.*
- **M4 — Playable tutorial + colony:** the Hauler start — spawn with one
  ship, work the contract board for credits, then buy an outpost/lease
  slots; build menu, contract/fleet management panels (in
  `galos_factory::ui`, shared with the standalone runner), time controls,
  power/upkeep/bottleneck dashboards. *Demo: haul → outpost →
  extractor→refinery→assembler on Sol bodies — same panels in both
  binaries.*
- **M5 — Economy + persistence:** price-curve markets seeded from listings,
  taxes/rent, state modifiers, loans/debt/bankruptcy, autosave/load. *Demo:
  full arc — haul, buy outpost on a loan, mine, refine, sell
  consumertechnology at Abraham Lincoln, reload save.*
- **M6 — Living galaxy:** 60s BGS refresh + diff ticker, conflict/piracy
  events, pending-state forecasts, second colony system, balance pass.
  *Demo: leave EDDN sync running, watch a real Boom move prices.*

### Verification (per milestone)

- `cargo test -p galos_factory` (tick, data-validation, seeding golden, save
  round-trip tests).
- `cargo run -p galos_factory --example headless` — production chain rates
  over 1000 ticks.
- With `DATABASE_URL` set: factory migrations apply cleanly alongside
  galos_db's (`set_ignore_missing`), snapshot loaders return real Sol data,
  env-gated commodity-id check passes.
- `cargo run -p galos_game` — end-to-end slice: select Sol, establish colony,
  build chain, sell, reload save.
