//! The metadata that rides beside the cell tree, kept without a database.
//!
//! `galos_db::index::metadata` does this from Postgres: it derives the names,
//! the populated table, the reaches, the supercharges, the factions and the
//! per-system body files, resumes them off a published directory, and patches
//! only the systems a pass touched. This is the same thing derived from
//! events instead — which is to say from
//! [`galos_journal::Galaxy`](galos_journal::Galaxy), which has already turned
//! them into records.
//!
//! Three things it does that a whole-table rebuild would not, and all three
//! are why it exists rather than the sink calling `Galaxy::names()` every
//! pass:
//!
//! - **It resumes.** A directory already published says most of what these
//!   tables hold, and a feed adds to it. Read back at startup, what was
//!   published stands; without that, following EDDN into an existing
//!   directory would republish it as whatever this session happened to hear.
//! - **It patches.** A pass touches tens of systems where the tables hold
//!   millions. `NameTable::publish` writes only the chunks that moved, and
//!   the tables that are written whole are written only when something in
//!   them changed.
//! - **It knows what a reach means when it is absent.** A system with nothing
//!   scanned has no entry, which is how the map tells "small" from "not on
//!   record". So a system whose scans this session has not heard keeps
//!   whatever the directory published for it, rather than being dropped.
//!
//! Factions are the one table that is written and never derived. A journal
//! names factions and numbers nothing — the ids are `galos_db`'s, minted on
//! write — so what is published stands untouched for the life of the run.
//! See `galos_journal::galaxy`.

use galos_index::meta::{
    Boost, Faction, PopulatedSystem, SystemBoost, SystemReach,
};
use galos_index::source::{
    self, boosts_path, factions_path, populated_path, reaches_path, write_meta,
};
use galos_index::NameTable;
use galos_journal::Galaxy;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use tracing::debug;

/// What one publish wrote, for the log.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wrote {
    /// Chunks of the names table rewritten, of however many it holds.
    pub name_chunks: usize,
    /// Whether each of the whole-file tables was rewritten.
    pub populated: bool,
    pub reaches: bool,
    pub boosts: bool,
    /// Only ever written whole: nothing here derives a faction id.
    pub factions: bool,
}

impl Wrote {
    /// Every table, whatever has changed.
    pub const EVERYTHING: Wrote = Wrote {
        name_chunks: 0,
        populated: true,
        reaches: true,
        boosts: true,
        factions: true,
    };
}

/// The metadata sidecars, held as the directory holds them.
///
/// Keyed by address rather than kept as the sorted vectors that are written,
/// since a pass patches a handful of systems and the sort is what a write
/// does at the end. The order is the format's — determinism, so the same
/// content is the same bytes — and not something to maintain between writes.
pub struct Tables {
    names: NameTable,
    populated: HashMap<i64, PopulatedSystem>,
    reaches: HashMap<i64, f32>,
    boosts: HashMap<i64, Boost>,
    /// Read back and written out untouched; nothing here can derive one.
    factions: Vec<Faction>,
    /// Which whole-file tables the directory has no file for at all.
    ///
    /// A table nothing in a run happened to move is a table never written,
    /// and a directory a follower has been filling for an hour can be
    /// missing one that way. To a client that absence is not "nothing to
    /// report": `galos_map` reads a missing supercharge table as "this
    /// index cannot say where a jet cone is" and refuses to plot a route
    /// for a drive that takes one. So the first write of a run writes
    /// whatever the directory lacks, empty if that is what it comes to.
    absent: Wrote,
}

impl Tables {
    /// What `dir` already publishes, or empty tables where it publishes
    /// nothing.
    ///
    /// Each table stands alone: a directory built before the supercharge
    /// table existed has every other part of it, and refusing to resume onto
    /// one would be refusing over a file whose absence the format already
    /// says is allowed. A table that is there and will not decode is a
    /// different thing and is an error, since carrying on would publish it
    /// back as empty.
    pub fn resume(dir: &Path) -> io::Result<Tables> {
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
        let absent = Wrote {
            name_chunks: 0,
            populated: populated.is_none(),
            reaches: reaches.is_none(),
            boosts: boosts.is_none(),
            factions: factions.is_none(),
        };
        let (populated, reaches, boosts, factions) = (
            populated.unwrap_or_default(),
            reaches.unwrap_or_default(),
            boosts.unwrap_or_default(),
            factions.unwrap_or_default(),
        );

        debug!(
            names = names.len(),
            populated = populated.len(),
            reaches = reaches.len(),
            boosts = boosts.len(),
            factions = factions.len(),
            dir = %dir.display(),
            "resumed the metadata tables",
        );
        Ok(Tables {
            names,
            populated: populated
                .into_iter()
                .map(|it| (it.address, it))
                .collect(),
            reaches: reaches
                .into_iter()
                .map(|it| (it.address, it.reach))
                .collect(),
            boosts: boosts
                .into_iter()
                .map(|it| (it.address, it.boost))
                .collect(),
            factions,
            absent,
        })
    }

    /// How many systems the names table holds.
    pub fn names(&self) -> usize {
        self.names.len()
    }

    /// Every address the names table holds.
    pub fn named(&self) -> HashSet<i64> {
        self.names.addresses().collect()
    }

    /// Drop the names of systems the cell tree does not hold, answering how
    /// many went.
    ///
    /// A repair and not an ordinary patch. The two halves of a directory
    /// stand for the same systems or it does not reopen, and a name whose
    /// system is not in the tree is a row the map can find and never draw.
    /// The feed publishes the system again soon enough; the row cannot be
    /// turned back into one.
    pub fn forget_names(&mut self, drawn: &HashSet<i64>) -> usize {
        let orphans: Vec<i64> = self
            .names
            .addresses()
            .filter(|address| !drawn.contains(address))
            .collect();
        for address in &orphans {
            self.names.remove(*address);
        }
        orphans.len()
    }

    /// Take what `galaxy` now says about `touched`, answering what moved.
    ///
    /// In memory. The tables that are single files are left for
    /// [`Self::write`], which is what decides between "what moved" and "all
    /// of it", and the per-system body files belong to the galaxy's own store
    /// (`galos_journal::bodies`), which is what writes them.
    ///
    /// A system the galaxy has nothing to say about is left exactly as the
    /// directory has it. Nothing here removes an entry for want of hearing
    /// about it: a reach whose scans this session did not see is not a system
    /// that shrank.
    pub fn patch(
        &mut self,
        galaxy: &Galaxy,
        touched: &HashSet<i64>,
    ) -> io::Result<Wrote> {
        let mut wrote = Wrote::default();

        for &address in touched {
            if let Some(entry) = galaxy.name_of(address) {
                // `upsert` leaves a chunk alone where nothing in it changed,
                // which is what keeps a hundred-megabyte table from being
                // rewritten because one system was named.
                self.names.upsert(entry);
            }

            match galaxy.populated_of(address) {
                Some(said) => {
                    let it = over(self.populated.get(&address), said);
                    if self.populated.get(&address) != Some(&it) {
                        self.populated.insert(address, it);
                        wrote.populated = true;
                    }
                }
                // Nothing. This cannot tell a system that has emptied from
                // one nobody has said anything about, so it withdraws
                // neither.
                //
                // `Population` is absent from an arrival in an unpopulated
                // system and `zero_is_none` turns a reported zero into the
                // same absence (`elite_journal::system`), so "no population
                // in hand" is the answer for a system that never had one,
                // one that has emptied, and one merely named by a passing
                // route. Withdrawing on that took the politics off every
                // system anybody plotted through, in a directory the
                // database had built.
                //
                // A row that really should go is withdrawn by the
                // derivation that can tell: `galos_db::index` re-reads
                // `population > 0` from the row itself, on a catch-up or
                // on `--only populated`.
                None => {}
            }

            if let Some(reach) = galaxy.reach_of(address) {
                if self.reaches.get(&address) != Some(&reach) {
                    self.reaches.insert(address, reach);
                    wrote.reaches = true;
                }
            }

            if let Some(boost) = galaxy.boost_of(address) {
                if self.boosts.get(&address) != Some(&boost) {
                    self.boosts.insert(address, boost);
                    wrote.boosts = true;
                }
            }
        }
        Ok(wrote)
    }

    /// Write the tables `moved` names, the ones the directory has no file
    /// for at all, and the names table's changed chunks.
    ///
    /// [`Wrote::EVERYTHING`] writes the lot, which is what a directory being
    /// published from nothing wants: a table that never moved was never
    /// written at all, and the factions table can only ever be written this
    /// way, nothing here being able to derive one.
    ///
    /// A missing file is written once whatever moved, because absence means
    /// something to a client: no supercharge table is "this index cannot
    /// say", not "no jet cones", and the map refuses a supercharged route
    /// over it. A follower filling a directory from a feed would otherwise
    /// leave one missing until the first system that happened to move it.
    pub fn write(&mut self, dir: &Path, moved: Wrote) -> io::Result<Wrote> {
        std::fs::create_dir_all(dir)?;
        let wrote = Wrote {
            name_chunks: 0,
            populated: moved.populated || self.absent.populated,
            reaches: moved.reaches || self.absent.reaches,
            boosts: moved.boosts || self.absent.boosts,
            factions: moved.factions || self.absent.factions,
        };
        if wrote.populated {
            self.write_populated(dir)?;
        }
        if wrote.reaches {
            self.write_reaches(dir)?;
        }
        if wrote.boosts {
            self.write_boosts(dir)?;
        }
        if wrote.factions {
            write_meta(&factions_path(dir), &self.factions)?;
        }
        self.absent = Wrote::default();
        Ok(Wrote { name_chunks: self.names.publish(dir)?, ..wrote })
    }

    /// The populated table, address-ordered as the format asks.
    fn write_populated(&self, dir: &Path) -> io::Result<()> {
        let mut table: Vec<PopulatedSystem> =
            self.populated.values().cloned().collect();
        table.sort_by_key(|it| it.address);
        write_meta(&populated_path(dir), &table)
    }

    fn write_reaches(&self, dir: &Path) -> io::Result<()> {
        let mut table: Vec<SystemReach> = self
            .reaches
            .iter()
            .map(|(&address, &reach)| SystemReach { address, reach })
            .collect();
        table.sort_by_key(|it| it.address);
        write_meta(&reaches_path(dir), &table)
    }

    fn write_boosts(&self, dir: &Path) -> io::Result<()> {
        let mut table: Vec<SystemBoost> = self
            .boosts
            .iter()
            .map(|(&address, &boost)| SystemBoost { address, boost })
            .collect();
        table.sort_by_key(|it| it.address);
        write_meta(&boosts_path(dir), &table)
    }
}

/// What an event says about a system, over what the directory publishes.
///
/// A row derived from events is thinner than one derived from the database
/// and always will be: a journal names factions and numbers none of them,
/// so [`galos_journal::Galaxy`] publishes an empty faction list by
/// construction, and the body counts arrive in their own events rather than
/// with the arrival. Writing such a row straight over a published one took
/// the faction ids off every populated system a feed happened to mention —
/// a thousand of them in the directory this was found in — and the map
/// colours and filters by exactly those.
///
/// So the event wins where it says something and what stands is kept where
/// it does not, which is the rule the database's own write path states
/// column by column and the rule this side already follows for a scan
/// arriving after an arrival.
fn over(
    published: Option<&PopulatedSystem>,
    said: PopulatedSystem,
) -> PopulatedSystem {
    let Some(stood) = published else { return said };
    PopulatedSystem {
        security: said.security.or(stood.security),
        government: said.government.or(stood.government),
        allegiance: said.allegiance.or(stood.allegiance),
        primary_economy: said.primary_economy.or(stood.primary_economy),
        secondary_economy: said.secondary_economy.or(stood.secondary_economy),
        // Never stated by an event, so never taken away by one.
        factions: match said.factions.is_empty() {
            true => stood.factions.clone(),
            false => said.factions,
        },
        body_count: said.body_count.or(stood.body_count),
        non_body_count: said.non_body_count.or(stood.non_body_count),
        ..said
    }
}

/// A published table, or [`None`] where the directory has no such file.
///
/// The format's own rule read back: a sidecar an older build never wrote is
/// an absence rather than a failure, and everything beside it is still good.
fn optional<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> io::Result<Option<T>> {
    match source::read_meta(path) {
        Ok(it) => Ok(Some(it)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}
