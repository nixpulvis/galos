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
use std::collections::HashMap;
use std::io;
use std::path::Path;

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
}
