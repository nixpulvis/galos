//! The metadata tables that ride beside the cell tree, held as a directory
//! holds them.
//!
//! The names, the populated systems, the reaches and the factions, and
//! whatever tables the program's dependents contribute
//! ([`crate::codec::tables::TableSet`]). Both derivations of the index keep
//! them all open across a run and patch them per pass, through this.
//!
//! What differs between the two is not here:
//!
//! - **Where a row comes from.** One side reads `systems`, the other asks a
//!   [`Galaxy`](crate::accumulate::galaxy::Galaxy), the event path's
//!   accumulator.
//! - **What an absence means.** The database re-reads `population > 0` and
//!   so can withdraw a row that stopped qualifying. A feed cannot: no
//!   population in hand is the answer for a system that never had one, one
//!   that has emptied, and one merely named by a passing route, so it
//!   withdraws nothing and leaves what the directory published. Hence the
//!   setters and the takers below are separate calls.
//!
//! The tables are held rather than rebuilt so that a run can **resume** onto
//! what a directory already publishes, **patch** the tens of systems a pass
//! touches rather than rewrite millions of rows, and **keep an absence
//! meaningful**: a system with nothing scanned has no reach, which is how
//! the map tells "small" from "not on record".
//!
//! Each table remembers whether it has moved since it was written
//! ([`Keyed`]), so a pass patches what it patches and [`Sidecars::write`]
//! writes what that moved, however many chunks the pass ran in.

use crate::codec::layout::names_dir;
use crate::codec::names::Names;
use crate::codec::rows::{RUN_BYTES, Sheet};
use crate::codec::tables::{
    Keyed, OpenTable, Spill, TableSet, each_row, sort_table,
};
use crate::codec::{Directory, naming};
use crate::records::{Faction, NameEntry, PopulatedSystem, SystemReach};
use crate::system::System;
use std::io;
use std::path::{Path, PathBuf};

impl Keyed<PopulatedSystem> {
    /// The populated table, `populated.bin`, held empty.
    pub fn populated() -> Keyed<PopulatedSystem> {
        Keyed::new("populated", populated_key)
    }
}

impl Keyed<SystemReach> {
    /// The reaches table, `reaches.bin`, held empty.
    pub fn reaches() -> Keyed<SystemReach> {
        Keyed::new("reaches", reach_key)
    }
}

impl Keyed<Faction> {
    /// The factions table, `factions.bin`, held empty: keyed by id, so
    /// written in id order.
    pub fn factions() -> Keyed<Faction> {
        Keyed::new("factions", faction_key)
    }
}

fn populated_key(row: &PopulatedSystem) -> i64 {
    row.address
}

fn reach_key(row: &SystemReach) -> i64 {
    row.address
}

fn faction_key(row: &Faction) -> i64 {
    row.id as i64
}

/// How many rows each table holds, for whatever reports a publish.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub names: usize,
    pub populated: usize,
    pub reaches: usize,
    pub factions: usize,
    /// Each contributed table's, by name.
    pub contributed: Vec<(&'static str, usize)>,
}

/// The metadata tables, held open across a run.
///
/// Each is the authority on what stands in the published directory: a pass
/// sets the changed systems' rows, and [`Self::write`] writes whichever
/// tables moved. Held rather than re-derived, deriving any of them being a
/// read of every system there is.
#[derive(Debug)]
pub struct Sidecars {
    names: Names,
    populated: Keyed<PopulatedSystem>,
    reaches: Keyed<SystemReach>,
    /// The faction names. Ids come from a sequence and a name is never
    /// rewritten, so this only ever grows.
    factions: Keyed<Faction>,
    contributed: Vec<Box<dyn OpenTable>>,
}

impl Sidecars {
    /// Nothing published yet, with `tables` beside the index's own.
    pub fn empty(tables: &TableSet) -> Sidecars {
        Sidecars {
            names: Names::default(),
            populated: Keyed::populated(),
            reaches: Keyed::reaches(),
            factions: Keyed::factions(),
            contributed: tables.iter().map(|it| it.empty()).collect(),
        }
    }

    /// What `dir` already publishes, with `tables` beside the index's own.
    ///
    /// Each table stands alone: a directory built before a table existed has
    /// every other part of it, and its absence is a thing the format allows.
    /// A table that is there and will not decode is an error — read as
    /// empty, it would be republished from the handful of addresses one
    /// pass touches and the rows already published would be gone.
    ///
    /// An absent table is remembered, and written by a [`Self::write`] only
    /// once [`Self::claim_absent`] has asked for it: no table means "this
    /// index cannot say" to a reader, and a caller decides whether it can
    /// say better.
    pub fn resume(dir: &Path, tables: &TableSet) -> io::Result<Sidecars> {
        Ok(Sidecars {
            names: Names::open(dir)?,
            populated: Keyed::resume("populated", populated_key, dir)?,
            reaches: Keyed::resume("reaches", reach_key, dir)?,
            factions: Keyed::resume("factions", faction_key, dir)?,
            contributed: tables
                .iter()
                .map(|it| it.resume(dir))
                .collect::<io::Result<_>>()?,
        })
    }

    /// Have the next write publish every table the directory had no file
    /// for, empty if that is what it comes to.
    pub fn claim_absent(&mut self) {
        self.populated.claim();
        self.reaches.claim();
        self.factions.claim();
        for table in &mut self.contributed {
            table.claim();
        }
    }

    /// Have the next write publish every table, moved or not: what a
    /// directory published from nothing wants.
    pub fn touch_all(&mut self) {
        self.populated.touch();
        self.reaches.touch();
        self.factions.touch();
        for table in &mut self.contributed {
            table.touch();
        }
    }

    /// Whether any table written whole has moved since it was written.
    pub fn moved(&self) -> bool {
        self.populated.moved()
            || self.reaches.moved()
            || self.factions.moved()
            || self.contributed.iter().any(|it| it.moved())
    }

    /// Append what the names table has taken and write whichever whole
    /// tables moved, answering how many name rows were appended.
    ///
    /// Everything else a caller needs for its own report is [`Self::counts`].
    /// The per-system body files are not here: one side writes them from the
    /// rows it read and the other from the store the galaxy keeps them in.
    pub fn write(&mut self, dir: &Path) -> io::Result<usize> {
        std::fs::create_dir_all(dir).map_err(naming(dir))?;
        self.populated.publish(dir)?;
        self.reaches.publish(dir)?;
        self.factions.publish(dir)?;
        for table in &mut self.contributed {
            table.publish(dir)?;
        }
        self.names.publish(dir)
    }

    /// Fold the names log into its base where it has grown long enough to
    /// be worth the rewrite, answering whether it did.
    ///
    /// Called after [`write`](Self::write), never before: the fold reads
    /// the directory, so what has been taken has to be in it first. The
    /// table is re-opened onto the generation the fold wrote, the one it
    /// held having just been unlinked.
    ///
    /// Rare by design — see [`crate::codec::names::Delta::worth_folding`]. The
    /// rewrite is the whole base, which is minutes at 200 M systems, and
    /// the log reaches the threshold about monthly on the live feed.
    pub fn compact_names(&mut self, dir: &Path) -> io::Result<bool> {
        if !self.names.worth_compacting() {
            return Ok(false);
        }
        // The fold is a build's road — scratch, sort runs, a generation —
        // and any of it can fail, so what is named is the table and not
        // the file.
        let names = names_dir(dir);
        Directory::at(dir).compact_names().map_err(naming(&names))?;
        self.names = Names::open(dir).map_err(naming(&names))?;
        Ok(true)
    }

    /// How many rows each table holds.
    pub fn counts(&self) -> Counts {
        Counts {
            names: self.names.len(),
            populated: self.populated.len(),
            reaches: self.reaches.len(),
            factions: self.factions.len(),
            contributed: self
                .contributed
                .iter()
                .map(|it| (it.name(), it.len()))
                .collect(),
        }
    }

    /// Put a system's name and place in the table.
    ///
    /// Nothing is written where the table already says exactly this, so a
    /// system reported again costs one binary search into a mapping.
    pub fn name(&mut self, entry: NameEntry) {
        self.names.name(entry);
    }

    /// Take a system out of the names table, answering whether it was there.
    pub fn unname(&mut self, address: i64) -> bool {
        self.names.unname(address)
    }

    /// Every address the names table holds.
    pub fn named(&self) -> impl Iterator<Item = i64> + '_ {
        self.names.addresses()
    }

    /// Whether the names table holds a row for `address`.
    ///
    /// The question a reader checking the directory's two halves against
    /// each other asks once per system, and the reason [`Self::named`] is
    /// not the way to ask it: collecting a galaxy's addresses to look one
    /// up is 5–8 GB at 200 M, where this is a binary search into a mapping
    /// — ~28 page touches and no allocation at all.
    pub fn names_hold(&self, address: i64) -> bool {
        self.names.name_of(address).is_some()
    }

    /// What the directory publishes for a system, where it publishes one.
    ///
    /// For a caller merging a thinner row over a richer one; see
    /// `galos::sink::tables`.
    pub fn published(&self, address: i64) -> Option<&PopulatedSystem> {
        self.populated.get(address)
    }

    /// How far a system reaches, where the directory publishes a row.
    ///
    /// [`Self::published`]'s twin, and for the same caller: a merge has to
    /// tell a directory that holds no reach for a system from one that
    /// holds a different reach, and the setters cannot say — [`Self::reach`]
    /// answers whether a write *changed* something, which is `true` in both
    /// cases. Reading the table off the disk again instead would double the
    /// hold on rows already in hand, and `reaches.bin` is 2.6 GB and 76 M
    /// rows over a galaxy.
    pub fn reach_of(&self, address: i64) -> Option<f32> {
        self.reaches.get(address).map(|it| it.reach)
    }

    /// Put a populated system's row in the table, answering whether that
    /// changed it.
    pub fn populate(&mut self, system: PopulatedSystem) -> bool {
        self.populated.put(system)
    }

    /// Take a system out of the populated table, answering whether it was
    /// there.
    pub fn depopulate(&mut self, address: i64) -> bool {
        self.populated.remove(address)
    }

    /// Record how far a system reaches, answering whether that changed it.
    pub fn reach(&mut self, address: i64, reach: f32) -> bool {
        self.reaches.put(SystemReach { address, reach })
    }

    /// Take a system out of the reaches table, answering whether it was
    /// there.
    pub fn unreach(&mut self, address: i64) -> bool {
        self.reaches.remove(address)
    }

    /// Take what each contributed table derives from a system's record,
    /// answering whether any of them changed; see [`Table::derive`].
    ///
    /// [`Table::derive`]: crate::codec::tables::Table::derive
    pub fn contribute(&mut self, system: &System) -> bool {
        let mut changed = false;
        for table in &mut self.contributed {
            changed |= table.contribute(system);
        }
        changed
    }

    /// Take a system out of every contributed table, answering whether any
    /// held it.
    pub fn uncontribute(&mut self, address: i64) -> bool {
        let mut changed = false;
        for table in &mut self.contributed {
            changed |= table.remove(address);
        }
        changed
    }

    /// Take the rows another directory's contributed tables publish, where
    /// `take` says to, answering how many rows changed; see
    /// [`Keyed::carry`].
    pub fn carry_contributed(
        &mut self,
        from: &Path,
        take: &dyn Fn(i64, bool) -> bool,
    ) -> io::Result<u64> {
        let mut changed = 0;
        for table in &mut self.contributed {
            changed += table.carry(from, take)?;
        }
        Ok(changed)
    }

    /// Add factions named since the last time, answering whether any were.
    ///
    /// Ids come from a sequence and a name on record is never rewritten, so
    /// the table only ever grows and the caller reads past the highest id
    /// it has.
    pub fn add_factions(&mut self, named: Vec<Faction>) -> bool {
        let mut changed = false;
        for faction in named {
            changed |= self.factions.put(faction);
        }
        changed
    }

    /// The highest faction id the table holds, or nought where it holds none.
    pub fn highest_faction(&self) -> i32 {
        self.factions.rows().map(|it| it.id).max().unwrap_or(0)
    }
}

/// The tables a record can fill, written as the records arrive.
///
/// [`Sidecars`] holds every row, which is right for a run that patches tens
/// of systems a pass and wrong for a build reading a galaxy: a row a
/// system is 22.4 MiB over a seven-day slice and some 6 GiB over the whole
/// of it, carried for the length of the read. So a row goes to a file as
/// it is derived, the way a name goes to a chunk, and the tables are made
/// from those files when the read is over — which is also what lets a
/// stopped build be taken up, the files being cut where the build's own
/// spills are. The directory is the caller's own, beside the build's
/// scratch rather than inside it: the build clears its scratch when it
/// finishes, and these have to outlive that by exactly as long as it takes
/// to write the tables.
///
/// The sort is not held either. [`finish`](Self::finish) sorts the rows a
/// run at a time and merges the runs, so what the tables cost to write is
/// one run rather than one galaxy — see `sort_table`.
///
/// Nothing is read back while the rows are being written, so a row does not
/// merge over a published one. That is the same argument
/// [`crate::accumulate::bodies::OnDisk::raising`] makes: a build from nothing
/// can only be told back what it has just said, and a dump names each system
/// once. For the same reason a contributed table is only ever pushed to
/// here, never taken from: there is nothing yet to take a row out of.
pub struct TableWriter {
    dir: PathBuf,
    populated: Sheet,
    reaches: Sheet,
    contributed: Vec<Box<dyn Spill>>,
}

impl TableWriter {
    /// Write the rows of a build into `dir`, from nothing, with `tables`
    /// beside the index's own.
    pub fn writing(dir: &Path, tables: &TableSet) -> io::Result<TableWriter> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)?;
        Ok(TableWriter {
            dir: dir.to_owned(),
            populated: Sheet::open(dir.join("populated.rows"))?,
            reaches: Sheet::open(dir.join("reaches.rows"))?,
            contributed: tables
                .iter()
                .map(|it| it.spill(dir))
                .collect::<io::Result<_>>()?,
        })
    }

    /// The same, seeded with the tables `served` already publishes.
    ///
    /// What a build carrying on from a read a stop published starts with:
    /// the rows it derived are in those tables and nowhere else, and the
    /// tables are written whole, so a second publish that had only this
    /// run's rows would take every earlier system's politics away.
    ///
    /// Read a row at a time and pushed straight back out to the spill, so
    /// carrying on costs a row rather than a table: a galaxy's published
    /// reaches are one array of tens of millions of rows, and decoding it
    /// into a `Vec` to walk it once would put the whole thing in memory
    /// for the length of the seeding.
    pub fn onto(
        dir: &Path,
        served: &Path,
        tables: &TableSet,
    ) -> io::Result<TableWriter> {
        let mut rows = TableWriter::writing(dir, tables)?;
        each_row(&Directory::at(served).table_path("populated"), |row| {
            rows.populate(&row)
        })?;
        each_row(
            &Directory::at(served).table_path("reaches"),
            |row: SystemReach| rows.reach(row.address, row.reach),
        )?;
        for table in &mut rows.contributed {
            table.seed(served)?;
        }
        Ok(rows)
    }

    /// One row of the populated table.
    pub fn populate(&mut self, row: &PopulatedSystem) -> io::Result<()> {
        self.populated.push(row)
    }

    /// How far one system reaches.
    pub fn reach(&mut self, address: i64, reach: f32) -> io::Result<()> {
        self.reaches.push(&SystemReach { address, reach })
    }

    /// What each contributed table derives from one system's record.
    pub fn contribute(&mut self, system: &System) -> io::Result<()> {
        for table in &mut self.contributed {
            table.contribute(system)?;
        }
        Ok(())
    }

    /// Everything pushed, on disk.
    pub fn flush(&mut self) -> io::Result<()> {
        self.populated.flush()?;
        self.reaches.flush()?;
        for table in &mut self.contributed {
            table.flush()?;
        }
        Ok(())
    }

    /// Sort the rows into their tables and drop them.
    ///
    /// The last row for an address wins, which is what a build carrying on
    /// from a published table leaves: the table's row goes in first and
    /// whatever this read said about the same system goes over it.
    ///
    /// Called once a build has published, never before: the rows are the
    /// only copy until this runs, and a table written over a directory whose
    /// build then stopped would stand for a galaxy nothing published.
    pub fn finish(self, dir: &Path) -> io::Result<Counts> {
        self.sorted(dir, RUN_BYTES)
    }

    /// The same, with the run size said outright, which is what lets a
    /// test spill several runs out of a handful of rows.
    fn sorted(mut self, dir: &Path, budget: usize) -> io::Result<Counts> {
        let at = self.dir.clone();
        self.flush()?;
        let mut counts = Counts {
            populated: sort_table::<PopulatedSystem>(
                self.populated.path(),
                &at,
                "populated",
                &Directory::at(dir).table_path("populated"),
                |it| it.address,
                budget,
            )?,
            reaches: sort_table::<SystemReach>(
                self.reaches.path(),
                &at,
                "reaches",
                &Directory::at(dir).table_path("reaches"),
                |it| it.address,
                budget,
            )?,
            ..Counts::default()
        };
        for table in std::mem::take(&mut self.contributed) {
            let name = table.name();
            counts.contributed.push((name, table.finish(&at, dir, budget)?));
        }
        drop(self);
        std::fs::remove_dir_all(&at)?;
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::tables::msgpack::read_meta;
    use crate::codec::tables::testing::{Cone, arriving, tables};
    use crate::core::star::StarKind;

    /// A table stays moved until it is written, and is not written again
    /// until something moves it
    ///
    /// A pass over what changed runs in chunks and writes each whole table
    /// once at the end. A later chunk that moved nothing must not unsay an
    /// earlier one, which would leave the published table standing for a
    /// galaxy the table in memory no longer agrees with; and a write must
    /// not leave a table standing moved, or every quiet pass rewrites it.
    #[test]
    fn a_table_is_moved_from_a_change_until_it_is_written() {
        let at = Scratch::new("moved");
        let mut held = Sidecars::empty(&tables());
        assert!(!held.moved(), "nothing had moved");

        held.populate(populated(1));
        held.populate(populated(1));
        assert!(held.moved(), "a later chunk unsaid the first one's row");

        held.write(&at.0).expect("the tables write");
        assert!(!held.moved(), "a written table stood moved");
        assert!(Directory::at(&at.0).table_path("populated").exists());
        assert!(
            !Directory::at(&at.0).table_path("reaches").exists(),
            "a table nothing moved was written"
        );
    }

    /// A row that reads as the one held is not a change
    ///
    /// The common case by far: a feed reports the same systems over and
    /// over, and the bytes already published must not be rewritten. The
    /// contributed tables are held to the same rule, and a record their
    /// table has nothing to say about takes out the row that stood.
    #[test]
    fn a_row_that_has_not_changed_moves_nothing() {
        let mut held = Sidecars::empty(&tables());

        assert!(held.populate(populated(1)), "the first row was not a change");
        assert!(!held.populate(populated(1)), "the same row read as a change");
        assert!(held.reach(1, 4.0), "the first reach was not a change");
        assert!(!held.reach(1, 4.0), "the same reach read as a change");

        let cone = arriving(1, StarKind::Neutron);
        assert!(held.contribute(&cone));
        assert!(!held.contribute(&cone), "the same record moved it");
        assert!(
            held.contribute(&System { position: [1., 2., 4.], ..cone }),
            "a corrected place was not a change"
        );
        assert!(
            held.contribute(&arriving(1, StarKind::G)),
            "a system that stopped qualifying kept its row"
        );
        assert!(!held.uncontribute(1), "the row was still there to take");

        assert!(held.depopulate(1), "the row was not there to withdraw");
        assert!(!held.depopulate(1), "withdrawing nothing was a change");
    }

    /// A contributed row is a function of the record and nothing else
    ///
    /// A record that no longer says what star a ship arrives at takes out
    /// the row its earlier kind gave, because a directory rebuilt from that
    /// record would have none. Kept current and rebuilt, the two directories
    /// hold the same rows.
    #[test]
    fn a_record_that_forgets_its_star_takes_its_row_out() {
        let mut kept = Sidecars::empty(&tables());
        assert!(kept.contribute(&arriving(1, StarKind::Neutron)));
        assert!(
            kept.contribute(&arriving(1, StarKind::Unknown)),
            "an unknown star left the row its earlier kind gave"
        );

        let mut rebuilt = Sidecars::empty(&tables());
        rebuilt.contribute(&arriving(1, StarKind::Unknown));
        assert!(!kept.uncontribute(1), "the kept table still held a row");
        assert!(!rebuilt.uncontribute(1), "the rebuilt table held a row");
    }

    /// An absent table is written only once a caller claims it
    ///
    /// No file says "this index cannot tell"; an empty one says "there are
    /// none". A resumed run leaves the absence standing unless it asks, and
    /// asked, writes the table even though no row of it moved.
    #[test]
    fn an_absent_table_is_written_once_claimed() {
        let at = Scratch::new("absent");
        let mut held =
            Sidecars::resume(&at.0, &tables()).expect("a resume of nothing");
        held.populate(populated(1));
        held.write(&at.0).expect("the tables write");
        assert!(
            !Directory::at(&at.0).table_path("cones").exists(),
            "an absence was written over"
        );

        held.claim_absent();
        held.write(&at.0).expect("the tables write");
        let cones: Vec<Cone> =
            read_meta(&Directory::at(&at.0).table_path("cones"))
                .expect("the claimed table");
        assert!(cones.is_empty());

        held.claim_absent();
        assert!(!held.moved(), "a table written once was absent again");
    }

    /// Somewhere to spill rows, removed with the value.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let at = std::env::temp_dir().join(format!(
                "galos-rows-{}-{}",
                std::process::id(),
                name
            ));
            let _ = std::fs::remove_dir_all(&at);
            std::fs::create_dir_all(&at).expect("a scratch directory");
            Scratch(at)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn populated(address: i64) -> PopulatedSystem {
        PopulatedSystem {
            address,
            name: format!("Sys {address}").into(),
            position: [0.0; 3],
            population: 1_000,
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
            state: None,
            power: None,
            powerplay_state: None,
        }
    }

    /// A build from records writes an empty table where it can and leaves
    /// out the one it cannot fill
    ///
    /// The two say different things to a reader: no contributed table is
    /// "this index cannot say", where an empty one is "there are none". A
    /// derivation from records can say the second of every table it
    /// derives, and only the first of the factions, whose ids are minted on
    /// a database write.
    #[test]
    fn a_build_from_records_leaves_the_factions_table_absent() {
        let at = Scratch::new("derived");
        let dir = at.0.join("served");
        std::fs::create_dir_all(&dir).expect("a directory");

        let rows =
            TableWriter::writing(&at.0.join("rows"), &tables()).expect("rows");
        let counts = rows.finish(&dir).expect("the tables write");

        assert_eq!(counts.populated, 0);
        assert_eq!(counts.contributed, vec![("cones", 0)]);
        assert!(
            Directory::at(&dir).table_path("populated").exists(),
            "no populated table"
        );
        assert!(
            Directory::at(&dir).table_path("reaches").exists(),
            "no reaches table"
        );
        assert!(
            Directory::at(&dir).table_path("cones").exists(),
            "no contributed table"
        );
        assert!(
            !Directory::at(&dir).table_path("factions").exists(),
            "an empty factions table says the galaxy has none"
        );
        assert!(
            !at.0.join("rows").exists(),
            "the rows outlived the tables made from them",
        );
    }

    /// A build carrying on keeps the rows the one before it published
    ///
    /// The tables are written whole, so a second publish holding only the
    /// second read's rows would take the politics off every system the
    /// first one read — which is a map that can draw a system and not
    /// colour it. The read that carries on says what it says over the top
    /// of what stands, which is the same rule the feed's own tables follow.
    #[test]
    fn rows_carry_on_from_the_table_that_was_published() {
        let at = Scratch::new("carried");
        let (spill, dir) = (at.0.join("rows"), at.0.join("served"));
        std::fs::create_dir_all(&dir).expect("a directory");

        let mut rows = TableWriter::writing(&spill, &tables()).expect("rows");
        rows.populate(&populated(1)).expect("a row");
        rows.reach(1, 4.0).expect("a reach");
        rows.contribute(&arriving(1, StarKind::Neutron)).expect("a cone");
        rows.populate(&populated(2)).expect("a row");
        let first = rows.finish(&dir).expect("the tables write");
        assert_eq!(first.populated, 2);

        let mut rows = TableWriter::onto(&spill, &dir, &tables())
            .expect("the tables back");
        rows.populate(&populated(3)).expect("a row");
        rows.reach(3, 16.0).expect("a reach");
        rows.contribute(&arriving(3, StarKind::Neutron)).expect("a cone");
        rows.contribute(&arriving(4, StarKind::G)).expect("no cone");
        // The same system again, as a resumed read re-deriving the line it
        // stopped on would: the newer row wins and there is still one of it.
        rows.reach(1, 5.0).expect("a reach");
        let counts = rows.finish(&dir).expect("the tables write");

        assert_eq!(counts.populated, 3, "the published rows were dropped");
        assert_eq!(counts.reaches, 2);
        assert_eq!(counts.contributed, vec![("cones", 2)]);

        let table: Vec<PopulatedSystem> =
            read_meta(&Directory::at(&dir).table_path("populated"))
                .expect("the populated table");
        assert_eq!(
            table.iter().map(|it| it.address).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "the table is not in address order",
        );
        let reaches: Vec<SystemReach> =
            read_meta(&Directory::at(&dir).table_path("reaches"))
                .expect("the reaches table");
        assert_eq!(
            reaches.iter().find(|it| it.address == 1).map(|it| it.reach),
            Some(5.0),
            "the older reach won",
        );
    }

    /// The sort is the sort, however many runs it takes
    ///
    /// The tables are written through an external sort — runs of rows
    /// sorted in memory, then merged — and the rule it has to keep is the
    /// one a map of every row would keep for free: address order, and the
    /// last row an address has winning. The place to lose it is a duplicate
    /// that falls either side of a run boundary, so this pushes rows in
    /// no order, repeats three of them, and sets the run size to one byte:
    /// every row is its own run and every duplicate straddles a boundary.
    #[test]
    fn a_sorted_table_is_what_a_map_of_every_row_would_have_written() {
        let at = Scratch::new("sorted");
        let dir = at.0.join("served");
        std::fs::create_dir_all(&dir).expect("a directory");

        let mut rows =
            TableWriter::writing(&at.0.join("rows"), &tables()).expect("rows");
        let pushed = [5i64, 3, 9, 3, 1, 9, 7, 3, 2, 8, 4, 6];
        for (n, &address) in pushed.iter().enumerate() {
            // The population says which push this row was, so the table
            // says which one won.
            let row =
                PopulatedSystem { population: n as u64, ..populated(address) };
            rows.populate(&row).expect("a row");
            rows.reach(address, n as f32).expect("a reach");
        }
        let counts = rows.sorted(&dir, 1).expect("the tables write");
        assert_eq!(counts.populated, 9, "a duplicate was written twice");
        assert_eq!(counts.reaches, 9);

        let table: Vec<PopulatedSystem> =
            read_meta(&Directory::at(&dir).table_path("populated"))
                .expect("the populated table");
        assert_eq!(
            table.iter().map(|it| it.address).collect::<Vec<_>>(),
            (1..=9).collect::<Vec<_>>(),
            "the merge did not leave the table in address order",
        );
        let won = |address: i64| {
            table
                .iter()
                .find(|it| it.address == address)
                .map(|it| it.population)
        };
        assert_eq!(won(3), Some(7), "an older row beat the newest one");
        assert_eq!(won(9), Some(5), "an older row beat the newest one");
        assert_eq!(won(5), Some(0));

        let reaches: Vec<SystemReach> =
            read_meta(&Directory::at(&dir).table_path("reaches"))
                .expect("the reaches table");
        assert_eq!(
            reaches.iter().find(|it| it.address == 3).map(|it| it.reach),
            Some(7.0),
            "an older reach beat the newest one",
        );

        // And what it wrote is what the whole-table writer would have: the
        // array is streamed a row at a time, so its header is the one
        // thing a reader could be handed differently.
        let bytes = std::fs::read(Directory::at(&dir).table_path("populated"))
            .expect("the table");
        assert_eq!(
            bytes,
            rmp_serde::to_vec(&table).expect("the table encodes"),
            "the streamed table is not the bytes a held one would be",
        );
    }
}
