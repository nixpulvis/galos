//! What the map holds of the index: the cell aggregates, the tables
//! read whole beside them, and the seam they are all read through.
//!
//! Read once behind a loading screen ([`load`]) and kept current while a
//! `--watch` build publishes under it ([`refresh`]).

pub(crate) mod load;
pub(crate) mod names;
pub(crate) mod refresh;

use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::meta::{Faction as MetaFaction, NameEntry, PopulatedSystem};
use galos_index::names::{Delta, Table};
use galos_index::{Index, Inhabitance, Source as IndexSource, SystemName};
use std::collections::HashMap;
use std::sync::Arc;

/// The seam the map reads cells and metadata through.
///
/// One transport for both, filesystem today and HTTP one day, so the whole of
/// it swaps at once rather than a cell path and a metadata path drifting onto
/// different backends. Cloneable, being an [`Arc`], so a fetch task takes a
/// handle onto its own thread.
#[derive(Resource, Clone)]
pub struct Transport(pub Arc<dyn IndexSource>);

/// The build directory the index was read from, for the diagnostics panel.
#[derive(Resource)]
pub struct IndexDir(pub String);

/// The cell aggregates, resident and read by every walk without a fetch.
#[derive(Resource)]
pub struct ResidentIndex(pub Index);

/// The dynamic set: a populated system's political columns, keyed by address.
///
/// About 96,000 systems against 129 million, held resident because a color
/// and a filter are asked of every drawn system every frame and neither can
/// wait on a fetch. A system absent here is ungoverned, which is most of them.
#[derive(Resource, Default, Clone)]
pub struct Populated(pub Arc<HashMap<i64, PopulatedSystem>>);

/// What each cell carries about the systems anybody lives in: the political
/// aggregation the field splats from.
///
/// Derived rather than fetched. It is [`Populated`] rolled up the tree
/// [`ResidentIndex`] already holds — one pass over a table that is resident
/// anyway — so a political field at any zoom costs no fetch and no server.
/// Re-derived whenever either of the two moves, which [`refresh`] does.
///
/// Its own weighting and not a reading off [`ResidentIndex`]: a cell's stellar
/// moments are the wrong place and the wrong size for the colonies under it.
/// Measured over `.galos_index`, the root's inhabited centroid and its
/// count-weighted centroid are 12.5 kly apart and their spreads differ
/// tenfold, so a political splat laid on the stellar moments draws the bubble
/// out toward the galactic core. See [`galos_index::inhabited`].
#[derive(Resource, Default, Clone)]
pub struct Settled(pub Arc<Inhabitance>);

/// Every system's name, where it sits, and how far it reaches: what the map
/// knows about any system without asking for it.
///
/// Held whole rather than fetched, since a search reaches any name, a route
/// steps between any two positions, and every system in the sky is drawn at
/// the size its reach says. The positions here are the graph the router
/// walks, so routing needs nothing loaded past this.
///
/// **Mapped**, not decoded: the table is the file the index publishes, and
/// holding it is five `mmap` calls and the delta log — nothing of the 5.8 GB
/// base is resident until something is looked up. See [`names`] for what
/// this used to cost. Cheap to clone: both halves sit behind [`Arc`]s, so a
/// fetch task takes a handle and names and colours its systems off the main
/// thread.
#[derive(Resource, Default, Clone)]
pub struct Names {
    /// The published table: the mapped base, and the log of what the feed
    /// has said since it was written.
    ///
    /// The log *is* the overlay the map used to keep beside the table, and
    /// the precedence is the format's rather than the map's: the log answers
    /// first and a withdrawal in it hides a base row. A refresh folds the
    /// log's tail in ([`galos_index::Names::absorb`]) and never touches the
    /// base.
    pub table: galos_index::Names,
    /// How far each scanned system reaches, in metres, by address.
    ///
    /// Its own table on disk (`reaches.bin`) and its own packing here, since
    /// it is published whole as MessagePack and covers a fifth of the index
    /// against the name table's whole: a system with nothing scanned in it
    /// is absent, which is how the map tells "small" from "not on record"
    /// and stands in for the second.
    ///
    /// Replaced whole by a refresh rather than patched: a scan arrives and
    /// the system it is about grows, which is the one thing in here that
    /// really changes with the feed.
    pub reaches: Arc<names::Reaches>,
    /// The galaxy the places come out of, where there is one open.
    ///
    /// **The table names systems; it no longer places them.** A published
    /// row is an address and a name, and where a system sits is in the cell
    /// payload that owns it — so the one structure that can answer "where
    /// is this address" is the tree, and asking it is a sphere query the
    /// size of the boxel the address names ([`Names::placed`]).
    ///
    /// Held here rather than passed, because every caller that draws a
    /// system by address already holds this table: a searched system, a
    /// route's stops, the systems a filter's panel lists. Opened from the
    /// same directory the table is mapped from, so a table that can name a
    /// system is a table that can place it.
    pub sky: Option<Arc<galos_index::Sky>>,
}

/// Faction id to the name it is shown under, read whole and held.
#[derive(Resource, Default)]
pub struct Factions(pub HashMap<i32, String>);

impl Populated {
    /// The populated record for a system, if it is one.
    pub fn get(&self, address: i64) -> Option<&PopulatedSystem> {
        self.0.get(&address)
    }
}

impl Names {
    /// The table these rows make, with no published directory behind it.
    ///
    /// All overlay and no base: the rows go in as the delta log's words, and
    /// every question is answered off those. What a test builds a table
    /// from, since the real one is a file and a test has rows. A build or a
    /// running map never comes this way — see [`Self::packed`].
    pub fn reaching(
        entries: Vec<NameEntry>,
        reaches: Vec<galos_index::SystemReach>,
    ) -> Names {
        Names {
            table: galos_index::Names::of(Table::default(), Delta::of(entries)),
            reaches: Arc::new(names::Reaches::of(reaches)),
            sky: None,
        }
    }

    /// The same, over a galaxy the places come out of.
    ///
    /// What a test that draws a system by address builds: the rows say what
    /// is named, the tree says where it is, and the two agree because the
    /// fixture mints each address from the place it wants
    /// ([`crate::testing::boxel_at`]).
    pub fn over(
        sky: Arc<galos_index::Sky>,
        entries: Vec<NameEntry>,
        reaches: Vec<galos_index::SystemReach>,
    ) -> Names {
        Names { sky: Some(sky), ..Names::reaching(entries, reaches) }
    }

    /// The table as the index published it, with the reaches beside it.
    ///
    /// What the map opens with: [`galos_index::Names::open`] has mapped the
    /// base and read the log, so there is nothing here to build. See
    /// `loading::read`.
    pub fn packed(
        table: galos_index::Names,
        reaches: names::Reaches,
        sky: Option<Arc<galos_index::Sky>>,
    ) -> Names {
        Names { table, reaches: Arc::new(reaches), sky }
    }

    /// Where the system at `address` sits, in light years.
    ///
    /// **The galaxy's answer, not the table's.** The published row holds an
    /// address and a name; the place is in the cell payload that owns the
    /// system, and the address says which boxel to look in — so this is one
    /// sphere query the size of that boxel, 0.8 ms at the class most
    /// systems are and 5 ms at the largest. That is a lookup a click or a
    /// plot can afford and a per-frame sweep cannot, which is why the
    /// drawn galaxy comes from the LOD walk and this answers for the
    /// handful of systems named outright.
    ///
    /// The middle of the boxel where the galaxy cannot answer — none open,
    /// or a system the feed has named that no cell holds yet. Within half a
    /// boxel of the truth, which is what a table with no tree behind it can
    /// honestly say, and never nothing: an address always names a box.
    /// Production has a tree: the sky is opened from the directory the
    /// table is mapped from.
    pub fn placed(&self, address: i64) -> DVec3 {
        if let Some(sky) = self.sky.as_ref()
            && let Some(at) = sky.placed(address)
        {
            return DVec3::from(at);
        }
        DVec3::from(elite_journal::Boxel::of(address).place().0)
    }

    /// Fold a tail of the delta log in, the feed having appended to it.
    ///
    /// The whole of what a refresh does to the names table: the log's rows
    /// *are* the changes, so there is nothing to diff and nothing to
    /// rebuild. The base is untouched and the log is copied on write, so a
    /// fetch task holding a clone keeps reading the table it was handed. See
    /// [`crate::map::index::refresh`].
    pub fn absorb(&mut self, tail: galos_index::Delta) {
        self.table.absorb(tail);
    }

    /// How far into the delta log this table has read, in bytes.
    ///
    /// What a refresh hands the transport to be given the rows past it, so a
    /// pass that finds fifty arrivals reads fifty rows and not the log.
    pub fn read_to(&self) -> u64 {
        self.table.delta().read_to()
    }

    /// How far the system at `address` reaches, in metres, where anything in
    /// it has been scanned.
    pub fn reach(&self, address: i64) -> Option<f32> {
        self.reaches.get(address)
    }

    /// The entry for an address, if the table names it.
    ///
    /// Owned, the name being bytes in a mapping rather than a `String` of
    /// its own: what a caller holds it has to be given a copy of.
    pub fn get(&self, address: i64) -> Option<NameEntry> {
        self.table.entry_of(address)
    }

    /// The systems whose name *begins* with `query`, at most `limit`.
    ///
    /// One fold, of the query: every name in the table is upper case by
    /// construction ([`galos_index::SystemName`]), so the comparison is
    /// bytes against bytes over the mapping and allocates nothing until
    /// something is found. It used to lowercase *both sides of every
    /// comparison*, which over 131 M entries is 131 M allocations to answer
    /// one search.
    ///
    /// A prefix, and not a substring. Answering a substring means reading
    /// every name — 3.94 GB at 200 M, measured at 564 ms warm and 5.7 s
    /// cold, on this thread — and the pages it faulted in evicted the cell
    /// payloads being drawn from, so a search for `SOL` stalled the frame
    /// *and* the galaxy's reads. A prefix is a binary search of
    /// `byname.bin` and ~28 pages: measured 3.0 ms for `SOL` over the real
    /// 200,071,629-name table. See `galos_index::Table::matching`.
    ///
    /// The cap is applied in the index rather than by collecting the
    /// galaxy and sorting it down. What [`crate::map::search`] does on top is
    /// sort the few that come back by how near they are to where the
    /// camera looks.
    pub fn find(
        &self,
        query: &str,
        near: Option<DVec3>,
        limit: usize,
    ) -> Vec<NameEntry> {
        self.table.matching_near(
            SystemName::new(query).as_str(),
            near.map(|at| [at.x, at.y, at.z]),
            limit,
        )
    }

    /// Whether any system is named exactly `name`.
    pub fn names_exactly(&self, name: &str) -> bool {
        self.address(name).is_some()
    }

    /// The address of the system named exactly `name`.
    ///
    /// What a route's ends are resolved through: a route is plotted between
    /// two named systems, and the graph it walks is keyed by address. A
    /// binary search of the table's by-name order, where it was a scan of
    /// the galaxy — 11.3 s measured, four to six of them per plot, which is
    /// most of the minute a route used to take to start.
    pub fn address(&self, name: &str) -> Option<i64> {
        self.table.address_of(SystemName::new(name).as_str())
    }

    /// How many systems the table names, one the log has renamed counted
    /// once
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether the table names nothing at all
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

impl Factions {
    /// The name a faction id is shown under, if known.
    pub fn name(&self, id: i32) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }

    /// The factions whose names contain `query`, best first, up to `limit`.
    ///
    /// A linear scan of the resident table, which a typeahead can afford: it is
    /// asked when the user types, not every frame, and the table is a few tens
    /// of thousands of short strings.
    pub fn search(&self, query: &str, limit: usize) -> Vec<MetaFaction> {
        let needle = query.to_lowercase();
        let mut found: Vec<MetaFaction> = self
            .0
            .iter()
            .filter(|(_, name)| name.to_lowercase().contains(&needle))
            .map(|(id, name)| MetaFaction { id: *id, name: name.clone() })
            .collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found.truncate(limit);
        found
    }
}
