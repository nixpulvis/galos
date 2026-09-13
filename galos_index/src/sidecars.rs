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
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::marker::PhantomData;
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
/// The sort is not held either. [`finish`](Self::finish) sorts the rows a
/// run at a time and merges the runs, so what the tables cost to write is
/// one run rather than one galaxy — see [`sort_table`].
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

/// One table's rows, length-framed so a row half written is the end of
/// what the file stands for rather than a row read out of the wrong bytes.
struct Sheet {
    path: PathBuf,
    out: BufWriter<File>,
}

impl Sheet {
    /// Open `path`, empty.
    fn open(path: PathBuf) -> io::Result<Sheet> {
        let file = File::create(&path)?;
        Ok(Sheet { path, out: BufWriter::new(file) })
    }

    /// One row, its length ahead of it.
    fn push<T: serde::Serialize>(&mut self, row: &T) -> io::Result<()> {
        let bytes = rmp_serde::to_vec(row)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.out.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.out.write_all(&bytes)
    }

    /// Everything pushed, on disk.
    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// How many bytes of rows one sorted run holds.
///
/// What the sort costs in memory, and the only dial it has: a run is read
/// back, sorted and written out, and the runs are then merged. A galaxy's
/// six gigabytes of rows is tens of runs at this size, which is few enough
/// that the merge can scan their heads rather than heap them.
const RUN_BYTES: usize = 128 * 1024 * 1024;

/// A row file read a row at a time.
///
/// Nothing reads a row file twice, so the rows go past rather than in: the
/// whole point of writing them to a file was not to hold them.
struct Framed {
    inner: BufReader<File>,
    buf: Vec<u8>,
}

impl Framed {
    /// Open a row file, or answer [`None`] where there is not one.
    fn open(path: &Path) -> io::Result<Option<Framed>> {
        match File::open(path) {
            Ok(file) => Ok(Some(Framed {
                inner: BufReader::new(file),
                buf: Vec::new(),
            })),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The next row and what it took on disk, or the end of the file.
    ///
    /// A row half written is a row the build never marked, so a short read
    /// is the end of what the file stands for rather than a failure.
    fn next<T: DeserializeOwned>(&mut self) -> io::Result<Option<(T, usize)>> {
        let mut head = [0u8; 4];
        match self.inner.read_exact(&mut head) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
        let len = u32::from_le_bytes(head) as usize;
        self.buf.resize(len, 0);
        match self.inner.read_exact(&mut self.buf) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                return Ok(None);
            }
            Err(err) => return Err(err),
        }
        let row = rmp_serde::from_slice(&self.buf)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(Some((row, len + 4)))
    }
}

impl Rows {
    /// Write the rows of a build into `dir`, from nothing.
    pub fn writing(dir: &Path) -> io::Result<Rows> {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)?;
        Ok(Rows {
            dir: dir.to_owned(),
            populated: Sheet::open(dir.join("populated.rows"))?,
            reaches: Sheet::open(dir.join("reaches.rows"))?,
            boosts: Sheet::open(dir.join("boosts.rows"))?,
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
    pub fn onto(dir: &Path, served: &Path) -> io::Result<Rows> {
        let mut rows = Rows::writing(dir)?;
        each_row(&populated_path(served), |row: PopulatedSystem| {
            rows.populate(&row)
        })?;
        each_row(&reaches_path(served), |row: SystemReach| {
            rows.reach(row.address, row.reach)
        })?;
        each_row(&boosts_path(served), |row: SystemBoost| {
            rows.boost(row.address, row.boost)
        })?;
        Ok(rows)
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

    /// Everything pushed, on disk.
    pub fn flush(&mut self) -> io::Result<()> {
        self.populated.flush()?;
        self.reaches.flush()?;
        self.boosts.flush()
    }

    /// Sort the rows into the three tables and drop them.
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
        let counts = Counts {
            names: 0,
            populated: sort_table::<PopulatedSystem>(
                &self.populated.path,
                &at,
                "populated",
                &populated_path(dir),
                |it| it.address,
                budget,
            )?,
            reaches: sort_table::<SystemReach>(
                &self.reaches.path,
                &at,
                "reaches",
                &reaches_path(dir),
                |it| it.address,
                budget,
            )?,
            boosts: sort_table::<SystemBoost>(
                &self.boosts.path,
                &at,
                "boosts",
                &boosts_path(dir),
                |it| it.address,
                budget,
            )?,
            factions: 0,
        };
        drop(self);
        std::fs::remove_dir_all(&at)?;
        Ok(counts)
    }
}

/// Write one table from its rows, in address order, without the table ever
/// being in memory.
///
/// An external sort: runs of `budget` bytes are read back, sorted and
/// written out, and the runs are then merged. What it stands in for is a
/// map of every row the read derived — 22.4 MiB over a seven-day slice and
/// some 6 GiB over the galaxy, which was the last thing on this road that
/// the whole sky had to fit in.
///
/// The last row an address has still wins, and that survives the split
/// into runs: a run is a stretch of the row file, so every row in one is
/// older than every row in the next, and a stable sort leaves the rows
/// inside a run in the order they were written.
fn sort_table<T: Serialize + DeserializeOwned>(
    rows: &Path,
    scratch: &Path,
    name: &str,
    table: &Path,
    key: impl Fn(&T) -> i64,
    budget: usize,
) -> io::Result<usize> {
    let runs = spill_runs::<T>(rows, scratch, name, &key, budget)?;
    let merged = scratch.join(format!("{name}.sorted"));
    let count = merge::<T>(&runs, &merged, &key)?;
    write_table::<T>(table, &merged, count)?;
    for run in runs {
        let _ = std::fs::remove_file(run);
    }
    let _ = std::fs::remove_file(&merged);
    Ok(count)
}

/// Read the rows a run at a time, sort each run, and answer the runs.
fn spill_runs<T: Serialize + DeserializeOwned>(
    rows: &Path,
    scratch: &Path,
    name: &str,
    key: &impl Fn(&T) -> i64,
    budget: usize,
) -> io::Result<Vec<PathBuf>> {
    let mut runs = Vec::new();
    let Some(mut framed) = Framed::open(rows)? else {
        return Ok(runs);
    };
    let mut held: Vec<T> = Vec::new();
    let mut bytes = 0usize;
    let mut ended = false;
    while !ended {
        match framed.next::<T>()? {
            Some((row, width)) => {
                held.push(row);
                bytes += width;
            }
            None => ended = true,
        }
        if held.is_empty() || (!ended && bytes < budget) {
            continue;
        }
        // Stable, so the rows an address has keep the order they were
        // written in and the last of them is still the last.
        held.sort_by_key(|it| key(it));
        let path = scratch.join(format!("{name}.run{:04}", runs.len()));
        let mut run = Sheet::open(path.clone())?;
        for row in held.drain(..) {
            run.push(&row)?;
        }
        run.flush()?;
        runs.push(path);
        bytes = 0;
    }
    Ok(runs)
}

/// Merge sorted runs into one file in address order, the last row an
/// address has winning.
///
/// A scan over the runs' heads rather than a heap: a run is [`RUN_BYTES`]
/// and a galaxy's rows are gigabytes, so there are tens of runs and the
/// scan costs less than the code a heap would.
fn merge<T: Serialize + DeserializeOwned>(
    runs: &[PathBuf],
    out: &Path,
    key: &impl Fn(&T) -> i64,
) -> io::Result<usize> {
    let mut readers = Vec::new();
    let mut heads: Vec<Option<T>> = Vec::new();
    for run in runs {
        let mut framed = Framed::open(run)?.expect("a run just written");
        heads.push(framed.next::<T>()?.map(|(row, _)| row));
        readers.push(framed);
    }

    let mut sorted = Sheet::open(out.to_owned())?;
    let mut count = 0usize;
    loop {
        let Some(address) = heads.iter().flatten().map(key).min() else {
            break;
        };
        // The runs in order, so a later run's row is taken over an earlier
        // one's, and inside a run the last of a stretch over the first:
        // both are the one rule, that the last row written wins.
        let mut best: Option<T> = None;
        for (at, head) in heads.iter_mut().enumerate() {
            while head.as_ref().is_some_and(|it| key(it) == address) {
                best = head.take();
                *head = readers[at].next::<T>()?.map(|(row, _)| row);
            }
        }
        sorted.push(&best.expect("the address came off a head"))?;
        count += 1;
    }
    sorted.flush()?;
    Ok(count)
}

/// Write a sorted run of rows as the MessagePack array a reader expects.
///
/// The bytes [`write_meta`] would write and by the same road — beside the
/// file and renamed over it — but streamed: the array's length is known
/// before its elements are, so nothing past one row is held.
fn write_table<T: Serialize + DeserializeOwned>(
    path: &Path,
    rows: &Path,
    count: usize,
) -> io::Result<()> {
    use serde::Serializer as _;
    use serde::ser::SerializeSeq;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut out =
        rmp_serde::Serializer::new(BufWriter::new(File::create(&tmp)?));
    let mut seq = out
        .serialize_seq(Some(count))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    if let Some(mut framed) = Framed::open(rows)? {
        while let Some((row, _)) = framed.next::<T>()? {
            seq.serialize_element(&row)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        }
    }
    seq.end().map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    out.into_inner().flush()?;
    std::fs::rename(&tmp, path)
}

/// Read a published table back a row at a time.
///
/// A table is one MessagePack array, and `read_meta` decodes it into a
/// `Vec`: fine for a pass that patches tens of systems, and a galaxy's
/// worth of rows in memory for a run that only means to walk it once. This
/// hands each row over as it is decoded instead. An absent table is no
/// rows rather than a failure — a directory that has published no reaches
/// has nothing to seed a resumed read with.
fn each_row<T: DeserializeOwned>(
    path: &Path,
    take: impl FnMut(T) -> io::Result<()>,
) -> io::Result<()> {
    use serde::de::DeserializeSeed;

    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let mut failed = None;
    let mut de = rmp_serde::Deserializer::new(BufReader::new(file));
    let each = Each { take, failed: &mut failed, marker: PhantomData };
    each.deserialize(&mut de)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    match failed {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The seed [`each_row`] walks an array with.
///
/// A seed rather than a `Vec` because the point is not to have one. The
/// caller's error rides out in `failed`: serde's own error type is the
/// decoder's, and a row the caller could not write is not a row that
/// failed to decode.
struct Each<'a, T, F> {
    take: F,
    failed: &'a mut Option<io::Error>,
    marker: PhantomData<fn() -> T>,
}

impl<'de, T, F> serde::de::DeserializeSeed<'de> for Each<'_, T, F>
where
    T: DeserializeOwned,
    F: FnMut(T) -> io::Result<()>,
{
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        de: D,
    ) -> Result<(), D::Error> {
        de.deserialize_seq(self)
    }
}

impl<'de, T, F> serde::de::Visitor<'de> for Each<'_, T, F>
where
    T: DeserializeOwned,
    F: FnMut(T) -> io::Result<()>,
{
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a table of rows")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        mut self,
        mut seq: A,
    ) -> Result<(), A::Error> {
        // The array is read to its end even after a write has failed: what
        // is being read is a file the run still has to be able to say
        // something about, and half a decode is not a state serde defines.
        while let Some(row) = seq.next_element::<T>()? {
            if self.failed.is_none() {
                if let Err(err) = (self.take)(row) {
                    *self.failed = Some(err);
                }
            }
        }
        Ok(())
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

        let rows = Rows::writing(&at.0.join("rows")).expect("rows");
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

        let mut rows = Rows::writing(&spill).expect("rows");
        rows.populate(&populated(1)).expect("a row");
        rows.reach(1, 4.0).expect("a reach");
        rows.populate(&populated(2)).expect("a row");
        let first = rows.finish(&dir).expect("the tables write");
        assert_eq!(first.populated, 2);

        let mut rows = Rows::onto(&spill, &dir).expect("the tables back");
        rows.populate(&populated(3)).expect("a row");
        rows.reach(3, 16.0).expect("a reach");
        rows.boost(3, Boost::Neutron).expect("a boost");
        // The same system again, as a resumed read re-deriving the line it
        // stopped on would: the newer row wins and there is still one of it.
        rows.reach(1, 5.0).expect("a reach");
        let counts = rows.finish(&dir).expect("the tables write");

        assert_eq!(counts.populated, 3, "the published rows were dropped");
        assert_eq!(counts.reaches, 2);
        assert_eq!(counts.boosts, 1);

        let table: Vec<PopulatedSystem> =
            read_meta(&populated_path(&dir)).expect("the populated table");
        assert_eq!(
            table.iter().map(|it| it.address).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "the table is not in address order",
        );
        let reaches: Vec<SystemReach> =
            read_meta(&reaches_path(&dir)).expect("the reaches table");
        assert_eq!(
            reaches.iter().find(|it| it.address == 1).map(|it| it.reach),
            Some(5.0),
            "the older reach won",
        );
    }

    /// The sort is the sort, however many runs it takes
    ///
    /// The tables are written through an external sort now — runs of rows
    /// sorted in memory, then merged — and the rule it has to keep is the
    /// one a map of every row kept for free: address order, and the last
    /// row an address has winning. The place to lose it is a duplicate
    /// that falls either side of a run boundary, so this pushes rows in
    /// no order, repeats three of them, and sets the run size to one byte:
    /// every row is its own run and every duplicate straddles a boundary.
    #[test]
    fn a_sorted_table_is_what_a_map_of_every_row_would_have_written() {
        let at = Scratch::new("sorted");
        let dir = at.0.join("served");
        std::fs::create_dir_all(&dir).expect("a directory");

        let mut rows = Rows::writing(&at.0.join("rows")).expect("rows");
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
            read_meta(&populated_path(&dir)).expect("the populated table");
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
            read_meta(&reaches_path(&dir)).expect("the reaches table");
        assert_eq!(
            reaches.iter().find(|it| it.address == 3).map(|it| it.reach),
            Some(7.0),
            "an older reach beat the newest one",
        );

        // And what it wrote is what the whole-table writer would have: the
        // array is streamed a row at a time, so its header is the one
        // thing a reader could be handed differently.
        let bytes = std::fs::read(populated_path(&dir)).expect("the table");
        assert_eq!(
            bytes,
            rmp_serde::to_vec(&table).expect("the table encodes"),
            "the streamed table is not the bytes a held one would be",
        );
    }
}
