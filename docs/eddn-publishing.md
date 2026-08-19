# Publishing to EDDN

Notes for the work that `galos-sync` does not do yet: sending what a
commander's journal holds back to [EDDN][eddn], rather than only reading what
everyone else sends. Nothing here is implemented. It is written down because
the protocol has more obligations in it than the transport suggests, and
because most of the cost lands in this repository rather than in the gateway.

The `TODO` at the top of `src/bin/galos-sync/journal/mod.rs` is the short
version of this file.

[eddn]: https://github.com/EDCD/EDDN

## The protocol

A publisher does an HTTP/1.1 `POST` to `https://eddn.edcd.io:4430/upload/` --
non-standard port, trailing slash required, TLS only. Plain HTTP is a `400`,
and HTTP/2 is not supported, which matters because `reqwest` will negotiate it
by ALPN unless the client is built `.http1_only()`.

The body is one UTF-8 JSON object:

```json
{ "$schemaRef": "https://eddn.edcd.io/schemas/journal/1",
  "header":  { "uploaderID": "...", "softwareName": "...", "softwareVersion": "...",
               "gameversion": "...", "gamebuild": "..." },
  "message": { "...": "schema-shaped payload" } }
```

`Content-Type: application/json`, optionally gzipped with `Content-Encoding:
gzip`, and at most 1 MiB. Form-encoded bodies were dropped in 2022.
`gatewayTimestamp` is stamped by the gateway; a sender does not write one,
which is the one field `eddn::Header` has that an outbound header must not.

Replies are `200` with a body of `OK`, or:

| Code | Meaning | What to do |
| --- | --- | --- |
| 400 | Malformed, or failed schema validation | Never retry. Fix it. |
| 408 | Timed out | Retry after at least a minute |
| 413 | Over 1 MiB | Compress; otherwise drop |
| 426 | Schema version no longer accepted | Never retry. Update the reference. |
| 503 | Gateway unavailable | Retry after at least a minute |

The minute is a floor rather than a suggestion, and the gateway's stated rule
is that no data is better than bad data, and delayed good data better than
degrading the service for everyone else. So a sender queues and backs off; it
does not retry in a loop.

There is no registration and no approval. A sender picks a `softwareName` that
is unique and stays that way, bumps `softwareVersion` whenever the content of
its messages changes, and appends `/test` to the `$schemaRef` while developing
so that test traffic is separated from the live galaxy -- the same suffix
`eddn::Schema` already reads on the way in.

## What a sender owes beyond the transport

This is where the work is.

**Augmentation.** Every journal message must carry `StarSystem`, `StarPos` and
`SystemAddress`, including for the events the game writes without them. A
sender tracks those from the last `Location`, `FSDJump` or `CarrierJump`,
cross-checks them against whatever address the event itself carries, and drops
the message where the two disagree. That last part is not defensive
programming: the game pauses its journal and resumes it with events missing,
so a sender that trusts its own running position will eventually attach one
system's name to another system's scan.

**Stripping.** Every `*_Localised` key comes out, along with a blacklist of
per-commander fields that the schemas reject outright: `ActiveFine`,
`CockpitBreach`, `BoostUsed`, `FuelLevel`, `FuelUsed`, `JumpDist`, `Latitude`,
`Longitude`, `Wanted`, `IsNewEntry`, `NewTraitsDiscovered`, `Traits`,
`VoucherAmount`, and `HappiestSystem`, `HomeSystem`, `MyReputation` and
`SquadronFaction` under a faction.

**Flags.** `horizons` and `odyssey` come only from `LoadGame`. Where they
cannot be determined the key is omitted entirely -- not sent as `null`, and
not sent as `false`.

**Routing.** There are eighteen schemas and each event class has its own.
`journal/1` accepts a short enum of events (`Docked`, `FSDJump`, `Scan`,
`Location`, `SAASignalsFound`, `CarrierJump`, `CodexEntry`) and rejects the
rest; `fssdiscoveryscan`, `approachsettlement`, `navroute`,
`fsssignaldiscovered`, `scanbarycentre` and the others each take their own
reference and their own message shape. `commodity/3`, `outfitting/2`,
`shipyard/2` and `blackmarket/1` do not come from the `.log` files at all --
they come from `Market.json`, `Outfitting.json` and `Shipyard.json` beside
them, or from CAPI.

## What this repository needs

1. **A publish half for the `eddn` crate.** It is a subscriber today: a ZMQ
   SUB socket, zlib inflate, and `Deserialize` on everything. `reqwest` 0.12
   and `flate2` are already in the workspace lockfile by way of `edsm`, so the
   dependencies are cheap. Build the client `.http1_only()`.

2. **An outbound envelope.** `eddn::Header` requires `gatewayTimestamp` and
   has no `gameversion` or `gamebuild`, which is exactly inverted for a
   sender. A separate outbound type is cleaner than making four fields
   `Option`.

3. **Raw JSON as the payload.** Do not serialize through `Entry<Event>`. Round
   tripping through our types drops every field we do not model, and EDDN
   validates the whole event, so what arrives would be a quietly abridged
   version of what the game wrote. The line the game wrote is what goes out.
   The parsed entry is used to pick a schema and to compute the augmentation,
   and never to reconstruct the message.

4. **Two gaps in `elite_journal`.** `Entry::horizons` and `Entry::odyssey` are
   `#[serde(default)] bool`, which collapses "unknown" into `false`; EDDN wants
   the key omitted when unknown, so they want to be `Option<bool>`. And
   `LoadGame` models neither `gameversion` nor `build` nor `Odyssey`.
   `Fileheader` has `gameversion` and `build` already and is the primary
   source for both header fields.

5. **Augmentation state.** `gather_names` already builds `SystemAddress ->
   (name, coordinate)` across a whole directory and `replay` already orders
   every entry by timestamp. An import can therefore look forward where a live
   sender can only guess from what it has seen, which is why publishing from
   an imported directory is the easier target and the right first one. A live
   tailer is a second mode and a harder one.

6. **Header identity.** `uploaderID` is the per-directory `.galos-commander`
   or `--user` that the importer already resolves. `softwareName` is a fixed
   `galos`. `softwareVersion` is `CARGO_PKG_VERSION`.

7. **Send discipline.** A queue with the one minute floor, `400` and `426` as
   permanent drops, deduplication so that re-importing a directory does not
   republish it, and a flag that appends `/test` -- on by default until this
   is known to work.

8. **Two things to be careful about.** Nothing may be published from the
   database, or from messages received off EDDN; only from this commander's
   own journal. Anything else is laundering other people's data back at the
   network. And the market schemas need `Market.json`, `Outfitting.json` and
   `Shipyard.json` parsing, which does not exist yet -- our types model the
   shape EDDN sends rather than the shape the game writes. That is the `TODO`
   on `journal::Cli`.

## Warning about what we send and cannot read

Publishing raw JSON means sending events `elite_journal` does not model. That
is correct -- our coverage is not EDDN's business -- but it should not be
silent, because a message we publish and cannot parse is the earliest notice
we get that our types have fallen behind the game.

**The audit observes and never gates.** The raw JSON goes out regardless of
what the parse says. If a failed parse could hold back a send we would have
given our model a veto over EDDN, which is the coverage gating that sending
raw was meant to avoid. The parse runs beside the send and produces log lines,
nothing else.

### Three outcomes, not two

`Event` is `#[serde(tag = "event")]` with a `#[serde(other)] Other` variant,
and nothing in `elite_journal` sets `deny_unknown_fields`. So "did it parse"
has three answers and they are worth different volumes:

1. **`Err(_)`, a hard failure.** The tag matched one of our variants and the
   payload disagreed with the struct: a renamed field, a changed type, a
   required field the game stopped writing. A real bug in `elite_journal`, and
   the loud one.

   Note that the doc comment on `incremental::tests` overstates the
   neighbouring case: it says a struct disagreeing about a field name is
   "filed as `Event::Other` forever", but for an internally tagged enum a known
   tag with an unreadable payload is an error rather than a fallthrough.
   `Other` catches unknown tags only.

2. **`Ok(Event::Other)`, an event we do not model.** Expected, common, and not
   an alarm. This is the coverage number: counted and summarised, not warned
   about one at a time.

3. **`Ok(_)` but lossy.** Parsed, and we read only part of it. The silent case,
   since serde ignores unknown fields by default and a clean parse says nothing
   about how much of the event was consumed. `serde_ignored` is the crate for
   this, though `Entry` flattens its event, flatten buffers into a content map,
   and `serde_ignored`'s path tracking degrades across exactly that boundary.
   Treat 1 and 2 as the deliverable and this as a stretch -- the `assert_read`
   helper in `incremental::tests` already covers the same ground offline and
   more reliably.

### The reader cannot report any of this yet

`elite_journal::entry::parse_journal_file` ends in
`.filter_map(|line| line.map(|l| serde_json::from_str(&l)).ok()).flatten()`.
That `flatten` drops every `Err` on the floor: a line our types choke on
today vanishes with no error, no count, and no raw text kept. Outcome 1 is
already happening on import and is invisible.

So the prerequisite is that the reader yields the raw line beside the result
rather than silently dropping the failures. Publishing needs the raw line
anyway, so this is one change and not two. It is the existing
`// TODO: add result inside vec too`.

### Volume

An import replays thousands of entries, and one `warn!` per unreadable line
buries the run it is meant to inform. `eddn::reporter` already holds the
pattern: hold back, count what went unsaid, and fold the count into the next
report that is made. Key a counter on the schema, the event tag and the
outcome; emit at the end of a finite import ("published 412 messages we do not
read: FSSSignalDiscovered x300, Music x88, ..."), and on an interval for a
live tailer. Outcome 1 should still name the event and quote the line --
`eddn::Error::near` already does the window-around-the-offset work.

### Worth more on the way in

Auditing our own journal is a thin sample. The subscriber sees everyone's, and
drops `Message::Unmodeled` at `debug!` and `Event::Other` in silence. Running
the same audit on inbound EDDN costs nothing and gives a far better picture of
what `elite_journal` cannot read. Which argues for the audit being a function
of its own in the `eddn` crate -- something that takes a schema and a message
and says which of the three happened, testable without a socket or a journal
directory, in the spirit of the split `read_frame` already has -- rather than
something buried in the publish path.

## Where to start

The `parse_journal_file` change and the audit function with its tests. Both
are independent of the transport, both are useful before a single message is
sent, and the first of the two is a bug fix either way.
