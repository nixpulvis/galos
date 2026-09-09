//! A 3D Galaxy Map for `galos`
//!
//! ![](https://github.com/nixpulvis/galos/blob/master/galos_map/demo.gif?raw=true)
//!
//! Requires a built `galos_index` directory: the cell tree and the metadata
//! sidecars beside it, read through one [`galos_index::Source`].
use bevy::prelude::*;
use galos_index::meta::{
    Boost, Faction as MetaFaction, NameEntry, PopulatedSystem,
};
use galos_index::{Index, Source as IndexSource};
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
pub mod refresh;
pub(crate) mod ruled;
pub mod schedule;
pub mod search;
pub mod space;
pub mod systems;
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

/// Every system's name, where it sits, and how far it reaches: what the map
/// knows about any system without asking for it.
///
/// Held whole rather than fetched, since a search reaches any name, a route
/// steps between any two positions, and every system in the sky is drawn at
/// the size its reach says. The positions here are the graph the router walks,
/// so routing needs nothing loaded past this.
/// Cheap to clone: the tables sit behind [`Arc`]s so a fetch task can take a
/// handle and name and color its systems off the main thread.
#[derive(Resource, Default, Clone)]
pub struct Names {
    /// Every entry, the order the table was written in.
    ///
    /// The table as it was read, and never written to afterwards: a hundred
    /// megabytes and two and a half million entries, which is not a thing to
    /// rebuild because fifty systems were named. What has changed since is
    /// [`Self::fresh`].
    pub entries: Arc<Vec<NameEntry>>,
    /// Address to its entry, for the O(1) lookup a selection wants.
    pub by_address: Arc<HashMap<i64, usize>>,
    /// What the feed has named or renamed since the table was read
    ///
    /// The names table is published in chunks and a pass moves one of them —
    /// arrivals land in the tail — so a refresh reads that chunk and puts what
    /// differs here rather than rebuilding the table around it. Small by
    /// construction: the systems named since the map started, which is what a
    /// feed adds in a session and not what a galaxy holds.
    ///
    /// Read before [`Self::entries`], so a corrected name answers over the one
    /// the base was read with.
    pub fresh: Arc<HashMap<i64, NameEntry>>,
    /// How far each scanned system reaches, in metres, by address.
    ///
    /// Its own table on disk (`reaches.bin`) and its own map here, since it
    /// covers a fifth of the index against the name table's whole: a system
    /// with nothing scanned in it is absent, which is how the map tells "small"
    /// from "not on record" and stands in for the second.
    ///
    /// Six megabytes, so a refresh replaces this whole rather than patching
    /// it: a scan arrives and the system it is about grows, which is the one
    /// thing in here that really changes with the feed.
    pub reaches: Arc<HashMap<i64, f32>>,
}

/// Which systems can supercharge a drive, and on what, by address.
///
/// Four systems in a hundred, held resident because the router weighs it at
/// every step of a search: a route is plotted over the whole galaxy rather
/// than over what is drawn, so a fetch per step is not a thing that could
/// work. What a boost is worth is the drive's to say
/// ([`systems::route::graph::Drive`]); this is only where one can be had.
///
/// Whether the index publishes such a table at all is kept beside it. An
/// index built before the table existed, or one whose builder has not reached
/// it, reads as [`Boosts::absent`] rather than as a galaxy where nobody has a
/// jet cone — and a route for a supercharging ship is then a question the map
/// cannot answer, which [`crate::ui`] says rather than answering the unaided
/// one under the supercharged drive's name.
#[derive(Resource, Default, Clone)]
pub struct Boosts {
    by_address: Arc<HashMap<i64, Boost>>,
    /// Whether the index published the table this came from
    published: bool,
}

impl Boosts {
    /// What the system at `address` can supercharge, if anything.
    pub fn get(&self, address: i64) -> Option<Boost> {
        self.by_address.get(&address).copied()
    }

    /// The table keyed by address, as the published rows give it.
    pub fn of(rows: Vec<galos_index::SystemBoost>) -> Boosts {
        Boosts {
            by_address: Arc::new(
                rows.into_iter().map(|it| (it.address, it.boost)).collect(),
            ),
            published: true,
        }
    }

    /// No such table in the index, which is not the same as an empty one.
    pub fn absent() -> Boosts {
        Boosts::default()
    }

    /// Whether the index published a supercharge table at all
    ///
    /// False is "the map cannot say where a jet cone is", not "there are
    /// none". Rebuilt with `galos-db index --only boosts`.
    pub fn published(&self) -> bool {
        self.published
    }

    /// A table built from what is already keyed, for tests.
    #[cfg(test)]
    pub(crate) fn holding(by_address: HashMap<i64, Boost>) -> Boosts {
        Boosts { by_address: Arc::new(by_address), published: true }
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
    /// Build the resident table and its address index from the raw entries,
    /// with the reaches keyed by address alongside them.
    pub fn reaching(
        entries: Vec<NameEntry>,
        reaches: Vec<galos_index::SystemReach>,
    ) -> Names {
        let by_address =
            entries.iter().enumerate().map(|(i, e)| (e.address, i)).collect();

        Names {
            entries: Arc::new(entries),
            by_address: Arc::new(by_address),
            fresh: Arc::default(),
            reaches: Arc::new(
                reaches.into_iter().map(|it| (it.address, it.reach)).collect(),
            ),
        }
    }

    /// How far the system at `address` reaches, in metres, where anything in
    /// it has been scanned.
    pub fn reach(&self, address: i64) -> Option<f32> {
        self.reaches.get(&address).copied()
    }

    /// The entry for an address, if the table holds it.
    ///
    /// What the feed has named since is read first, so a system that arrived
    /// after the table did is found and a corrected name answers over the one
    /// on record when the map started. See [`Self::fresh`].
    pub fn get(&self, address: i64) -> Option<&NameEntry> {
        self.fresh.get(&address).or_else(|| {
            self.by_address.get(&address).map(|&i| &self.entries[i])
        })
    }

    /// Every entry the table names, the fresh ones in place of what they
    /// replace
    ///
    /// One pass over the base skipping whatever the overlay has an answer for,
    /// then the overlay itself, so a system named twice is listed once and it
    /// is the later name that shows.
    ///
    /// The skip is a hash lookup per entry over two and a half million of
    /// them, so it is not paid while the overlay is empty — which is every
    /// session until the feed names something, and every search in a session
    /// with no refresh behind it.
    pub fn iter(&self) -> impl Iterator<Item = &NameEntry> {
        let replaced = !self.fresh.is_empty();
        self.entries
            .iter()
            .filter(move |entry| {
                !replaced || !self.fresh.contains_key(&entry.address)
            })
            .chain(self.fresh.values())
    }

    /// The systems whose name contains `query`, case-insensitively.
    ///
    /// A linear scan, which a search action can afford: it is asked when the
    /// user types rather than every frame, and the table is a couple of million
    /// short strings.
    pub fn find(&self, query: &str) -> Vec<&NameEntry> {
        let needle = query.to_lowercase();
        self.iter()
            .filter(|e| e.name.to_lowercase().contains(&needle))
            .collect()
    }

    /// Whether any system is named exactly `name`, case-insensitively.
    pub fn names_exactly(&self, name: &str) -> bool {
        self.iter().any(|e| e.name.eq_ignore_ascii_case(name))
    }

    /// The address of the system named exactly `name`, case-insensitively.
    ///
    /// What a route's ends are resolved through: a route is plotted between two
    /// named systems, and the graph it walks is keyed by address.
    pub fn address(&self, name: &str) -> Option<i64> {
        self.iter()
            .find(|e| e.name.eq_ignore_ascii_case(name))
            .map(|e| e.address)
    }

    /// How many systems the table names, the fresh ones counted once
    pub fn len(&self) -> usize {
        self.entries.len()
            + self
                .fresh
                .keys()
                .filter(|address| !self.by_address.contains_key(address))
                .count()
    }

    /// Whether the table names nothing at all
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.fresh.is_empty()
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
    pub(crate) fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::ui::styled);
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
