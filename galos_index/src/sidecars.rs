//! The metadata tables that ride beside the cell tree, held as a directory
//! holds them.
//!
//! Five files: the names, the populated systems, the reaches, the
//! supercharges and the factions. Both derivations of the index keep all
//! five open across a run and patch them per pass, through this.
//!
//! What differs between the two is not here:
//!
//! - **Where a row comes from.** One side reads `systems`, the other asks a
//!   [`Galaxy`](crate::Galaxy), the event path's accumulator.
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

use crate::meta::{
    Boost, Faction, NameEntry, PopulatedSystem, SystemBoost, SystemReach,
};
use crate::names::NameTable;
use crate::source::{
    boosts_path, factions_path, populated_path, reaches_path, read_meta,
    write_meta,
};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Seek, Write};
use std::path::{Path, PathBuf};

/// Which of the tables written whole a pass moved.
///
/// Three of them are written whole or not at all, so only *that* some chunk
/// of a paged pass dirtied one is worth recording. A pass accumulates this
/// and writes each table it names once, after its last chunk.
///
/// The names table is not among them: it is chunked and tracks its own dirty
/// set, so [`Sidecars::write`] always asks it and it answers with the chunks
/// that moved.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Moved {
    pub populated: bool,
    pub reaches: bool,
    pub boosts: bool,
    pub factions: bool,
}

impl Moved {
    /// Every table, which is what a directory published from nothing wants.
    ///
    /// A table that never moved was never written at all, and the factions
    /// table can only ever be written this way by a derivation that cannot
    /// mint an id.
    pub const EVERYTHING: Moved =
        Moved { populated: true, reaches: true, boosts: true, factions: true };

    /// Take in what a further chunk of the same pass moved.
    ///
    /// A table stays dirty once any chunk has dirtied it: a later chunk that
    /// moved nothing cannot unsay what an earlier one changed, which would
    /// leave the published table standing for a galaxy the table in memory
    /// no longer agrees with.
    pub fn absorb(&mut self, other: Moved) {
        self.populated |= other.populated;
        self.reaches |= other.reaches;
        self.boosts |= other.boosts;
        self.factions |= other.factions;
    }

    /// Whether anything at all moved.
    pub fn any(&self) -> bool {
        self.populated || self.reaches || self.boosts || self.factions
    }
}

/// How many rows each table holds, for whatever reports a publish.
#[derive(Copy, Clone, Debug, Default)]
pub struct Counts {
    pub names: usize,
    pub populated: usize,
    pub reaches: usize,
    pub boosts: usize,
    pub factions: usize,
}

/// The five metadata tables, held open across a run.
///
/// Each is the authority on what stands in the published directory: a pass
/// sets the changed systems' rows, and [`Self::write`] writes whichever
/// tables moved. Held rather than re-derived, deriving any of them being a
/// read of every system there is.
///
/// Keyed by address rather than kept as the sorted vectors that are written:
/// a pass patches a handful of systems, and the sort is a write's own step,
/// the order being the format's so that the same content is the same bytes.
#[derive(Debug)]
pub struct Sidecars {
    names: NameTable,
    populated: HashMap<i64, PopulatedSystem>,
    reaches: HashMap<i64, f32>,
    boosts: HashMap<i64, Boost>,
    /// The faction names, in id order. Ids come from a sequence and a name is
    /// never rewritten, so this only ever grows.
    factions: Vec<Faction>,
}

impl Sidecars {
    /// Nothing published yet.
    pub fn empty() -> Sidecars {
        Sidecars::over(NameTable::default())
    }

    /// The tables of a full build, whose names table is the other half of
    /// the read the cell tree came out of, built as those rows arrived.
    pub fn building(names: NameTable) -> Sidecars {
        Sidecars::over(names)
    }

    fn over(names: NameTable) -> Sidecars {
        Sidecars {
            names,
            populated: HashMap::new(),
            reaches: HashMap::new(),
            boosts: HashMap::new(),
            factions: Vec::new(),
        }
    }

    /// What `dir` already publishes, and which of the tables it has no file
    /// for at all.
    ///
    /// Each table stands alone: a directory built before the supercharge
    /// table existed has every other part of it, and its absence is a thing
    /// the format allows. A table that is there and will not decode is an
    /// error — read as empty, it would be republished from the handful of
    /// addresses one pass touches and the rows already published would be
    /// gone.
    ///
    /// What is absent comes back as a [`Moved`], the answer to an absent
    /// table being to write it: no supercharge table means "this index
    /// cannot say" to a client, and the map refuses a supercharged route
    /// over it. A caller that expects all five to be there can refuse
    /// instead.
    pub fn resume(dir: &Path) -> io::Result<(Sidecars, Moved)> {
        let names = match NameTable::read(dir) {
            Ok(names) => names,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                NameTable::from_entries(Vec::new())
            }
            Err(err) => return Err(err),
        };
        let populated: Option<Vec<PopulatedSystem>> =
            optional(&populated_path(dir))?;
        let reaches: Option<Vec<SystemReach>> = optional(&reaches_path(dir))?;
        let boosts: Option<Vec<SystemBoost>> = optional(&boosts_path(dir))?;
        let factions: Option<Vec<Faction>> = optional(&factions_path(dir))?;

        let absent = Moved {
            populated: populated.is_none(),
            reaches: reaches.is_none(),
            boosts: boosts.is_none(),
            factions: factions.is_none(),
        };
        let held = Sidecars {
            names,
            populated: populated
                .unwrap_or_default()
                .into_iter()
                .map(|it| (it.address, it))
                .collect(),
            reaches: reaches
                .unwrap_or_default()
                .into_iter()
                .map(|it| (it.address, it.reach))
                .collect(),
            boosts: boosts
                .unwrap_or_default()
                .into_iter()
                .map(|it| (it.address, it.boost))
                .collect(),
            factions: factions.unwrap_or_default(),
        };
        Ok((held, absent))
    }

    /// Write the names chunks that moved and whichever whole tables `moved`
    /// names, answering how many name chunks were written.
    ///
    /// Everything else a caller needs for its own report is [`Self::counts`].
    /// The per-system body files are not here: one side writes them from the
    /// rows it read and the other from the store the galaxy keeps them in.
    pub fn write(&mut self, dir: &Path, moved: Moved) -> io::Result<usize> {
        std::fs::create_dir_all(dir)?;
        if moved.populated {
            write_populated(dir, &self.populated)?;
        }
        if moved.reaches {
            write_reaches(dir, &self.reaches)?;
        }
        if moved.boosts {
            write_boosts(dir, &self.boosts)?;
        }
        if moved.factions {
            write_meta(&factions_path(dir), &self.factions)?;
        }
        self.names.publish(dir)
    }

    /// How many rows each table holds.
    pub fn counts(&self) -> Counts {
        Counts {
            names: self.names.len(),
            populated: self.populated.len(),
            reaches: self.reaches.len(),
            boosts: self.boosts.len(),
            factions: self.factions.len(),
        }
    }

    /// Put a system's name and place in the table.
    ///
    /// `upsert` leaves a chunk alone where nothing in it changed, so naming
    /// one system does not rewrite a hundred-megabyte table.
    pub fn name(&mut self, entry: NameEntry) {
        self.names.upsert(entry);
    }

    /// Take a system out of the names table, answering whether it was there.
    pub fn unname(&mut self, address: i64) -> bool {
        self.names.remove(address)
    }

    /// Every address the names table holds.
    pub fn named(&self) -> impl Iterator<Item = i64> + '_ {
        self.names.addresses()
    }

    /// What the directory publishes for a system, where it publishes one.
    ///
    /// For a caller merging a thinner row over a richer one; see
    /// `galos-sync`'s `sink::tables`.
    pub fn published(&self, address: i64) -> Option<&PopulatedSystem> {
        self.populated.get(&address)
    }

    /// Put a populated system's row in the table, answering whether that
    /// changed it.
    ///
    /// A row that reads exactly as the one held changes nothing and writes
    /// nothing — the common case, a feed reporting the same systems over and
    /// over.
    pub fn populate(&mut self, system: PopulatedSystem) -> bool {
        match self.populated.get(&system.address) {
            Some(held) if *held == system => false,
            _ => {
                self.populated.insert(system.address, system);
                true
            }
        }
    }

    /// Take a system out of the populated table, answering whether it was
    /// there.
    pub fn depopulate(&mut self, address: i64) -> bool {
        self.populated.remove(&address).is_some()
    }

    /// Record how far a system reaches, answering whether that changed it.
    pub fn reach(&mut self, address: i64, reach: f32) -> bool {
        match self.reaches.get(&address) {
            Some(&held) if held == reach => false,
            _ => {
                self.reaches.insert(address, reach);
                true
            }
        }
    }

    /// Take a system out of the reaches table, answering whether it was
    /// there.
    pub fn unreach(&mut self, address: i64) -> bool {
        self.reaches.remove(&address).is_some()
    }

    /// Record what a system can supercharge, answering whether that changed
    /// it.
    pub fn boost(&mut self, address: i64, boost: Boost) -> bool {
        match self.boosts.get(&address) {
            Some(&held) if held == boost => false,
            _ => {
                self.boosts.insert(address, boost);
                true
            }
        }
    }

    /// Take a system out of the supercharge table, answering whether it was
    /// there.
    pub fn unboost(&mut self, address: i64) -> bool {
        self.boosts.remove(&address).is_some()
    }

    /// Add factions named since the last time, answering whether any were.
    ///
    /// Appended rather than merged: ids come from a sequence and a name on
    /// record is never rewritten, so the table only ever grows and the
    /// caller reads past the highest id it has.
    pub fn add_factions(&mut self, named: Vec<Faction>) -> bool {
        if named.is_empty() {
            return false;
        }
        self.factions.extend(named);
        true
    }

    /// The highest faction id the table holds, or nought where it holds none.
    pub fn highest_faction(&self) -> i32 {
        self.factions.iter().map(|it| it.id).max().unwrap_or(0)
    }
}

/// A published table, or [`None`] where the directory has no such file.
///
/// A sidecar an older build never wrote is an absence rather than a failure,
/// and everything beside it is still good.
fn optional<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> io::Result<Option<T>> {
    match read_meta(path) {
        Ok(table) => Ok(Some(table)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Write `populated.bin`: the dynamic set the map colours and navigates by,
/// in address order so the same table is always the same bytes.
///
/// Free of [`Sidecars`]: a build asked for one part alone holds none of the
/// tables, reading that part fresh and writing it.
pub fn write_populated(
    dir: &Path,
    populated: &HashMap<i64, PopulatedSystem>,
) -> io::Result<usize> {
    let mut table: Vec<&PopulatedSystem> = populated.values().collect();
    table.sort_unstable_by_key(|system| system.address);
    write_meta(&populated_path(dir), &table)?;
    Ok(table.len())
}

/// Write `reaches.bin`: how far each scanned system reaches, in address order
/// so the same table is always the same bytes.
pub fn write_reaches(
    dir: &Path,
    reaches: &HashMap<i64, f32>,
) -> io::Result<usize> {
    let mut table: Vec<SystemReach> = reaches
        .iter()
        .map(|(&address, &reach)| SystemReach { address, reach })
        .collect();
    table.sort_unstable_by_key(|it| it.address);
    write_meta(&reaches_path(dir), &table)?;
    Ok(table.len())
}

/// Write `boosts.bin`: which systems can supercharge a drive, in address
/// order so the same table is always the same bytes.
pub fn write_boosts(
    dir: &Path,
    boosts: &HashMap<i64, Boost>,
) -> io::Result<usize> {
    let mut table: Vec<SystemBoost> = boosts
        .iter()
        .map(|(&address, &boost)| SystemBoost { address, boost })
        .collect();
    table.sort_unstable_by_key(|it| it.address);
    write_meta(&boosts_path(dir), &table)?;
    Ok(table.len())
}

/// The three tables a record can fill, written as the records arrive.
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
/// What this does not do is the sort. The tables are written in address
/// order, so [`finish`](Self::finish) reads the rows back into maps and
/// sorts them, which is the galaxy's worth of rows in memory once rather
/// than throughout. Item 1 of `TODO-scale-regions.md` is the rest of it.
///
/// Nothing is read back while the rows are being written, so a row does
/// not merge over a published one. That is the same argument
/// [`crate::bodies::Published::raising`] makes: a build from nothing can
/// only be told back what it has just said, and a dump names each system
/// once.
pub struct Rows {
    dir: PathBuf,
    populated: Sheet,
    reaches: Sheet,
    boosts: Sheet,
}

/// One table's rows, length-framed so the file is always cut between two.
struct Sheet {
    path: PathBuf,
    out: BufWriter<File>,
    bytes: u64,
}

impl Sheet {
    /// Open `path`, at `from` bytes of it: everything past that was written
    /// after the mark being taken up, and is derived again by the read that
    /// takes it up.
    fn open(path: PathBuf, from: u64) -> io::Result<Sheet> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)?;
        file.set_len(from)?;
        let mut file = file;
        file.seek(io::SeekFrom::Start(from))?;
        Ok(Sheet { path, out: BufWriter::new(file), bytes: from })
    }

    /// One row, its length ahead of it.
    fn push<T: serde::Serialize>(&mut self, row: &T) -> io::Result<()> {
        let bytes = rmp_serde::to_vec(row)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.out.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.out.write_all(&bytes)?;
        self.bytes += 4 + bytes.len() as u64;
        Ok(())
    }

    /// Everything pushed, on disk, and how many bytes that is.
    fn flush(&mut self) -> io::Result<u64> {
        self.out.flush()?;
        Ok(self.bytes)
    }

    /// Every row back, in the order they were written.
    fn read<T: serde::de::DeserializeOwned>(&self) -> io::Result<Vec<T>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(err),
        };
        let mut rows = Vec::new();
        let mut at = 0usize;
        while at + 4 <= bytes.len() {
            let len = u32::from_le_bytes(
                bytes[at..at + 4].try_into().expect("four bytes"),
            ) as usize;
            at += 4;
            // A row half written is a row the build never marked, so it is
            // the end of what this file stands for.
            if at + len > bytes.len() {
                break;
            }
            rows.push(
                rmp_serde::from_slice(&bytes[at..at + len]).map_err(|e| {
                    io::Error::new(io::ErrorKind::InvalidData, e)
                })?,
            );
            at += len;
        }
        Ok(rows)
    }
}

impl Rows {
    /// Write the rows of a build into `dir`, taking up whatever `from`
    /// names.
    ///
    /// `from` is the three byte counts [`lengths`](Self::lengths) answered
    /// when the build last marked its place, or zeroes for a build starting
    /// over. A file is cut back to its figure, so the rows and the systems
    /// they stand for are cut at the same system.
    pub fn writing(dir: &Path, from: [u64; 3]) -> io::Result<Rows> {
        std::fs::create_dir_all(dir)?;
        Ok(Rows {
            dir: dir.to_owned(),
            populated: Sheet::open(dir.join("populated.rows"), from[0])?,
            reaches: Sheet::open(dir.join("reaches.rows"), from[1])?,
            boosts: Sheet::open(dir.join("boosts.rows"), from[2])?,
        })
    }

    /// Take what `galaxy` says about every system in `touched`.
    pub fn take(
        &mut self,
        galaxy: &crate::Galaxy,
        touched: &HashSet<i64>,
    ) -> io::Result<()> {
        for &address in touched {
            self.system(galaxy, address)?;
        }
        Ok(())
    }

    /// One system's rows, as the galaxy has them.
    pub fn system(
        &mut self,
        galaxy: &crate::Galaxy,
        address: i64,
    ) -> io::Result<()> {
        if let Some(row) = galaxy.populated_of(address) {
            self.populate(&row)?;
        }
        if let Some(reach) = galaxy.reach_of(address) {
            self.reach(address, reach)?;
        }
        if let Some(boost) = galaxy.boost_of(address) {
            self.boost(address, boost)?;
        }
        Ok(())
    }

    /// One row of the populated table.
    pub fn populate(&mut self, row: &PopulatedSystem) -> io::Result<()> {
        self.populated.push(row)
    }

    /// How far one system reaches.
    pub fn reach(&mut self, address: i64, reach: f32) -> io::Result<()> {
        self.reaches.push(&SystemReach { address, reach })
    }

    /// What one system's arrival star can supercharge.
    pub fn boost(&mut self, address: i64, boost: Boost) -> io::Result<()> {
        self.boosts.push(&SystemBoost { address, boost })
    }

    /// Everything pushed, on disk, and the byte counts a mark is made of.
    pub fn lengths(&mut self) -> io::Result<[u64; 3]> {
        Ok([
            self.populated.flush()?,
            self.reaches.flush()?,
            self.boosts.flush()?,
        ])
    }

    /// Read the rows back, write the three tables in address order, and
    /// drop the rows.
    ///
    /// The last row for an address wins, which is what a resumed read that
    /// derived a system twice would leave — it cannot, the cut being exact,
    /// but a map is what sorts them and a map has to answer something.
    ///
    /// Called once a build has published, never before: the rows are the
    /// only copy until this runs, and a table written over a directory whose
    /// build then stopped would stand for a galaxy nothing published.
    pub fn finish(mut self, dir: &Path) -> io::Result<Counts> {
        let at = self.dir.clone();
        self.lengths()?;
        let populated: HashMap<i64, PopulatedSystem> = self
            .populated
            .read::<PopulatedSystem>()?
            .into_iter()
            .map(|row| (row.address, row))
            .collect();
        let reaches: HashMap<i64, f32> = self
            .reaches
            .read::<SystemReach>()?
            .into_iter()
            .map(|row| (row.address, row.reach))
            .collect();
        let boosts: HashMap<i64, Boost> = self
            .boosts
            .read::<SystemBoost>()?
            .into_iter()
            .map(|row| (row.address, row.boost))
            .collect();
        let counts = Counts {
            names: 0,
            populated: write_populated(dir, &populated)?,
            reaches: write_reaches(dir, &reaches)?,
            boosts: write_boosts(dir, &boosts)?,
            factions: 0,
        };
        drop(self);
        std::fs::remove_dir_all(&at)?;
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table one chunk of a pass moved is still written for the pass
    ///
    /// A pass over what changed runs in chunks and writes each whole table
    /// once at the end, so what each chunk moved is accumulated. A later
    /// chunk that moved nothing must not unsay an earlier one, which would
    /// leave the published table standing for a galaxy the table in memory
    /// no longer agrees with.
    #[test]
    fn a_pass_keeps_every_table_one_of_its_chunks_moved() {
        let mut pass = Moved::default();
        pass.absorb(Moved { populated: true, ..Moved::default() });
        pass.absorb(Moved { reaches: true, ..Moved::default() });
        pass.absorb(Moved::default());

        assert!(
            pass.populated,
            "a chunk that moved nothing unsaid the first chunk's populated row"
        );
        assert!(
            pass.reaches,
            "a chunk that moved nothing unsaid the second chunk's reach"
        );
        assert!(!pass.boosts, "a table no chunk moved was written anyway");
        assert!(!pass.factions, "a table no chunk moved was written anyway");
        assert!(pass.any(), "a pass that moved two tables moved nothing");
    }

    /// A row that reads as the one held is not a change
    ///
    /// The common case by far: a feed reports the same systems over and
    /// over, and the bytes already published must not be rewritten.
    #[test]
    fn a_row_that_has_not_changed_moves_nothing() {
        let mut held = Sidecars::empty();
        let system = || PopulatedSystem {
            address: 1,
            name: "SOL".to_string(),
            position: [0.0; 3],
            population: 22_780_919_531,
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
        };

        assert!(held.populate(system()), "the first row was not a change");
        assert!(!held.populate(system()), "the same row read as a change");
        assert!(held.reach(1, 4.0), "the first reach was not a change");
        assert!(!held.reach(1, 4.0), "the same reach read as a change");
        assert!(held.boost(1, Boost::Neutron));
        assert!(!held.boost(1, Boost::Neutron), "the same boost moved it");

        assert!(held.depopulate(1), "the row was not there to withdraw");
        assert!(!held.depopulate(1), "withdrawing nothing was a change");
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
            name: format!("Sys {address}"),
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
        }
    }

    /// A build from records writes an empty table where it can and leaves
    /// out the one it cannot fill
    ///
    /// The two say different things to a client: no supercharge table is
    /// "this index cannot say where a jet cone is", where an empty one is
    /// "there are none". A derivation from records can say the second of
    /// the three tables it derives, and only the first of the factions,
    /// whose ids are minted on a database write.
    #[test]
    fn a_build_from_records_leaves_the_factions_table_absent() {
        let at = Scratch::new("derived");
        let dir = at.0.join("served");
        std::fs::create_dir_all(&dir).expect("a directory");

        let rows = Rows::writing(&at.0.join("rows"), [0; 3]).expect("rows");
        let counts = rows.finish(&dir).expect("the tables write");

        assert_eq!(counts.populated, 0);
        assert!(populated_path(&dir).exists(), "no populated table");
        assert!(reaches_path(&dir).exists(), "no reaches table");
        assert!(
            boosts_path(&dir).exists(),
            "no supercharge table, which the map reads as a refusal to plot"
        );
        assert!(
            !factions_path(&dir).exists(),
            "an empty factions table says the galaxy has none"
        );
        assert!(
            !at.0.join("rows").exists(),
            "the rows outlived the tables made from them",
        );
    }

    /// Rows are cut where the build marked, and taken up from there
    ///
    /// The whole of what makes a stopped build resumable: the rows and the
    /// systems they stand for are cut at the same system. A row past the
    /// cut is one the resumed read derives again, so leaving it would
    /// publish it twice; a row before the cut is one nothing derives again,
    /// so losing it is a system the map can draw and never colour.
    #[test]
    fn rows_are_cut_where_the_build_marked() {
        let at = Scratch::new("cut");
        let (spill, dir) = (at.0.join("rows"), at.0.join("served"));
        std::fs::create_dir_all(&dir).expect("a directory");

        let mut rows = Rows::writing(&spill, [0; 3]).expect("rows");
        rows.populate(&populated(1)).expect("a row");
        rows.reach(1, 4.0).expect("a reach");
        let marked = rows.lengths().expect("the lengths");

        // Past the mark: what a run writes between its last mark and the
        // stop that killed it.
        rows.populate(&populated(2)).expect("a row");
        rows.reach(2, 9.0).expect("a reach");
        rows.lengths().expect("the lengths");
        drop(rows);

        let mut rows = Rows::writing(&spill, marked).expect("taken up");
        rows.populate(&populated(3)).expect("a row");
        rows.reach(3, 16.0).expect("a reach");
        rows.boost(3, Boost::Neutron).expect("a boost");
        let counts = rows.finish(&dir).expect("the tables write");

        assert_eq!(counts.populated, 2, "the cut was not where it was marked");
        assert_eq!(counts.reaches, 2);
        assert_eq!(counts.boosts, 1);

        let table: Vec<PopulatedSystem> =
            read_meta(&populated_path(&dir)).expect("the populated table");
        assert_eq!(
            table.iter().map(|it| it.address).collect::<Vec<_>>(),
            vec![1, 3],
            "the rows past the mark were published",
        );
    }
}
