//! A 3D Galaxy Map for `galos`
//!
//! ![](https://github.com/nixpulvis/galos/blob/master/galos_map/demo.gif?raw=true)
//!
//! Requires a built `galos_index` directory: the cell tree and the metadata
//! sidecars beside it, read through one [`galos_index::Source`].
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::meta::{
    Boost, Faction as MetaFaction, NameEntry, PopulatedSystem,
};
use galos_index::names::{Delta, Table};
use galos_index::{Index, Inhabitance, Source as IndexSource, SystemName};
use std::collections::HashMap;
use std::sync::Arc;

// What `main.rs` stands the app up from, and no more than that. The binary is
// its own crate and reaches the map through this one, so a module is `pub`
// here exactly where the binary names it: each of these for its `plugin`, and
// `systems` for `route::graph` besides. `ruled` is not among them — the ruled
// plane goes up with `grid::plugin` — so it is held in, as its own submodules
// already are.
//
// Worth spelling out because the map has no library consumers: nothing but
// `main.rs` imports any of this, so `pub` past what it needs says a thing is
// API when it is not, and `cargo doc` starts asking why a public item explains
// itself in terms of private ones.
pub mod camera;
pub mod dev;
pub mod grid;
pub mod keys;
pub mod loading;
pub mod names;
pub mod refresh;
pub(crate) mod ruled;
pub mod schedule;
pub mod search;
pub mod shot;
pub mod space;
pub mod systems;
#[cfg(test)]
pub(crate) mod testing;
pub mod ui;

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

/// Which systems can supercharge a drive, where they are, and on what — in
/// address order, as published.
///
/// Two systems in a hundred, held resident because the router weighs it at
/// every step of a search: a route is plotted over the whole galaxy rather
/// than over what is drawn, so a fetch per step is not a thing that could
/// work. What a boost is worth is the drive's to say
/// ([`systems::route::graph::Drive`]); this is only where one can be had.
///
/// **The published rows themselves**, kept in their own order rather than
/// spread into a map. A `HashMap` over 3.8 M rows is some 140 MB of slots
/// to hold 60 MB of facts, and the order is what the second reader of this
/// table wants: the boost stars are the nodes of
/// [`systems::route::highway`]'s coarse graph, sorted into cells once and
/// then read where they lie. A lookup is therefore a binary search rather
/// than a hash — asked once per expansion, against a neighbour query that
/// measures hundreds of candidates, so the twenty-odd compares are noise
/// beside it.
///
/// The place comes with the row ([`galos_index::SystemBoost`]) and that is
/// the whole of why routing no longer touches the names table: finding
/// where four million cones sat used to mean walking the names table's
/// address column, 4 GB of mapping faulted and 7.9 s before a galactic
/// route could start planning.
///
/// Whether the index publishes such a table at all is kept beside it. An
/// index built before the table existed, or one whose builder has not reached
/// it, reads as [`Boosts::absent`] rather than as a galaxy where nobody has a
/// jet cone — and a route for a supercharging ship is then a question the map
/// cannot answer, which [`crate::ui`] says rather than answering the unaided
/// one under the supercharged drive's name.
#[derive(Resource, Default, Clone)]
pub struct Boosts {
    /// The rows, ascending by address.
    rows: Arc<Vec<galos_index::SystemBoost>>,
    /// Whether the index published the table this came from
    published: bool,
}

impl Boosts {
    /// What the system at `address` can supercharge, if anything.
    pub fn get(&self, address: i64) -> Option<Boost> {
        let at = self.rows.binary_search_by_key(&address, |row| row.address);
        Some(self.rows[at.ok()?].boost)
    }

    /// The table as the published rows give it.
    ///
    /// Sorted here rather than trusted: the builder writes it in address
    /// order and the lookup is a binary search, which is wrong rather than
    /// slow if a file says otherwise.
    pub fn of(rows: Vec<galos_index::SystemBoost>) -> Boosts {
        let mut rows = rows;
        if !rows.windows(2).all(|pair| pair[0].address <= pair[1].address) {
            rows.sort_unstable_by_key(|row| row.address);
        }
        Boosts { rows: Arc::new(rows), published: true }
    }

    /// No such table in the index, which is not the same as an empty one.
    pub fn absent() -> Boosts {
        Boosts::default()
    }

    /// Whether the index published a supercharge table at all
    ///
    /// False is "the map cannot say where a jet cone is", not "there are
    /// none". Rebuilt with `galos-index build --from database --only boosts`.
    pub fn published(&self) -> bool {
        self.published
    }

    /// The rows, for the coarse graph [`systems::route::highway`] sorts
    /// them into.
    pub(crate) fn table(&self) -> &[galos_index::SystemBoost] {
        &self.rows
    }
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
    /// [`crate::refresh`].
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
    /// galaxy and sorting it down. What [`crate::search`] does on top is
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

#[cfg(test)]
pub(crate) mod tests {
    use bevy_egui::egui;

    /// A context lettered as the map letters its own
    ///
    /// What is drawn here is measured, and how wide a word comes out is the
    /// font's answer. A test weighing a line against the room there is for it
    /// in a face the map does not use is a test about some other map.
    ///
    /// **And it complains about an id clash whether or not this is a debug
    /// build.** Egui's `warn_on_id_clash` defaults to
    /// `cfg!(debug_assertions)`, so in release it reports nothing — and the
    /// eighteen tests that draw a piece of the bar twice and assert egui
    /// said nothing were all *vacuously* green under `--release`, along
    /// with the two that check the instruments themselves can hear a real
    /// clash, which failed outright and are how it was noticed. It is a
    /// runtime option and not a compile-time one, so the answer is to ask
    /// for it: an id clash is a fault of the code and not of the profile it
    /// was built in.
    pub(crate) fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::ui::styled);
        ctx.options_mut(|options| options.warn_on_id_clash = true);
        ctx
    }

    /// Draw `contents` into a bare context and tessellate what it made
    ///
    /// Laying a widget out is not the half of it that goes wrong. Egui defers
    /// a color the caller did not give as a placeholder for the painter to
    /// answer, and one answered by another placeholder is caught nowhere until
    /// epaint meets it and panics. So the shapes are turned into triangles
    /// here, which is the step that looks.
    ///
    /// Shared by the chrome and the panels, which paint their rows the same
    /// way and can go wrong in it the same way.
    pub(crate) fn painted(mut contents: impl FnMut(&mut egui::Ui)) {
        let ctx = context();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));
        ctx.tessellate(output.shapes, output.pixels_per_point);
    }

    /// Every piece of text `contents` painted, in the order it was painted
    ///
    /// What a widget draws is the whole of what the user is told, and a row
    /// that lays its text out itself has no label to be asked what it says.
    /// So this reads it back off the shapes.
    pub(crate) fn words(
        mut contents: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        let ctx = context();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));

        fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().into()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_of(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut said = Vec::new();
        for shape in &output.shapes {
            text_of(&shape.shape, &mut said);
        }
        said
    }

    /// What egui said about `first` being redrawn as `second`
    ///
    /// Egui checks between one pass and the next whether a rectangle kept its
    /// place while everything in it changed identity, which is how a widget
    /// taking another's state shows up. It says so through `log` and nowhere
    /// else, so this listens for that rather than reading the painted output
    /// the way [`complaints`] does.
    ///
    /// Two passes over one context, since a warning about what changed
    /// between them cannot be had from either alone.
    ///
    /// What is heard is kept per thread, and a test hears its own thread and
    /// no other. A logger is installed once for the whole process and the
    /// tests it hears run at the same time as the rest, so a warning from
    /// somewhere else would otherwise be read as this pass having complained.
    /// Egui logs from whichever thread called it, which is this one.
    ///
    /// **Debug builds only, because the check itself is.** Egui's
    /// `warn_if_rect_changes_id` is `#[cfg(debug_assertions)]` — compiled
    /// out of a release build rather than switched off by an option, as
    /// `warn_on_id_clash` is ([`context`]) — so under `--release` there is
    /// nothing to listen for and every caller would assert that nothing
    /// was said about a check that never ran. Gated rather than left to
    /// pass vacuously: a test that cannot observe its subject should not
    /// be counted as having observed it.
    #[cfg(debug_assertions)]
    pub(crate) fn between_passes(
        first: impl FnMut(&mut egui::Ui),
        second: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        use std::thread::{self, ThreadId};

        /// What the logger has heard, by the thread that said it. Tests share
        /// a process, and a logger may be installed once in one.
        static HEARD: Mutex<Option<HashMap<ThreadId, Vec<String>>>> =
            Mutex::new(None);
        static LOGGER: OnceLock<()> = OnceLock::new();

        /// What `HEARD` has for `thread`, made if it has none
        fn heard_by<R>(
            thread: ThreadId,
            act: impl FnOnce(&mut Vec<String>) -> R,
        ) -> R {
            // Poisoning says some other test panicked mid-pass, which is that
            // test's news to break rather than this one's.
            let mut heard =
                HEARD.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            act(heard
                .get_or_insert_with(HashMap::new)
                .entry(thread)
                .or_default())
        }

        struct Listener;
        impl log::Log for Listener {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, record: &log::Record) {
                if record.level() <= log::Level::Warn {
                    let said = record.args().to_string();
                    heard_by(thread::current().id(), |heard| heard.push(said));
                }
            }
            fn flush(&self) {}
        }

        LOGGER.get_or_init(|| {
            let _ = log::set_boxed_logger(Box::new(Listener));
            log::set_max_level(log::LevelFilter::Warn);
        });

        let mine = thread::current().id();
        heard_by(mine, Vec::clear);

        let ctx = context();
        for pass in
            [Box::new(first) as Box<dyn FnMut(&mut egui::Ui)>, Box::new(second)]
        {
            let mut pass = pass;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| pass(ui));
        }

        heard_by(mine, std::mem::take)
    }

    /// What egui complained about in the margins while `contents` was drawn
    ///
    /// Egui reports two widgets sharing an id by painting the offending
    /// rectangle in its error color and writing what happened beside it. It
    /// says so nowhere else, so this picks it out of what was painted.
    pub(crate) fn complaints(
        contents: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        let mut said = words(contents);
        said.retain(|line| {
            line.contains("Double use") || line.contains("use of")
        });
        said
    }
}
