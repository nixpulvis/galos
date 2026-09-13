//! The metadata that rides beside the cell tree, kept from events.
//!
//! The tables themselves are [`Sidecars`], shared with
//! `galos_db::index::metadata`, which does this from Postgres. What is here
//! is the half that cannot be shared: where a row comes from, and what an
//! absence means.
//!
//! A row comes from [`galos_index::Galaxy`], which has already turned the
//! events into records. And an absence means nothing at all — which is the
//! whole difference between this and the database's side:
//!
//! - **Nothing is withdrawn.** This cannot tell a system that has emptied
//!   from one nobody has said anything about, so it withdraws neither.
//!   `Population` is absent from an arrival in an unpopulated system and
//!   `zero_is_none` turns a reported zero into the same absence
//!   (`elite_journal::system`), so "no population in hand" is the answer for
//!   a system that never had one, one that has emptied, and one merely named
//!   by a passing route. Withdrawing on that took the politics off every
//!   system anybody plotted through, in a directory the database had built.
//!   A row that really should go is withdrawn by the derivation that can
//!   tell: `galos_db::index` re-reads `population > 0` from the row itself,
//!   on a catch-up or on `--only populated`.
//! - **A thinner row merges over a richer one.** See `over`.
//!
//! Factions are the one table that is written and never derived. A journal
//! names factions and numbers nothing — the ids are `galos_db`'s, minted on
//! write — so what is published stands untouched for the life of the run.
//! See `galos_index::galaxy`.

use galos_index::meta::PopulatedSystem;
use galos_index::sidecars::{Counts, Moved, Sidecars};
use galos_index::Galaxy;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use tracing::debug;

/// What one publish wrote, for the log.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wrote {
    /// Rows appended to the names table's delta log.
    ///
    /// The log is that table's unit of change: a pass that named fifty
    /// systems appends fifty rows and rewrites nothing at all, so what is
    /// worth reporting is rows and not files. Nought where nothing was
    /// named, which is the ordinary quiet pass.
    pub name_rows: usize,
    /// Whether this publish folded the log into a fresh base.
    ///
    /// A fold rewrites every row the table names — minutes at 200 M
    /// systems — and happens about monthly on the live feed, so it is
    /// reported rather than left silent. See `galos_index::names::compact`.
    pub folded: bool,
    /// Which of the whole-file tables were rewritten.
    pub tables: Moved,
}

impl Wrote {
    /// Every table, whatever has changed.
    pub const EVERYTHING: Wrote =
        Wrote { name_rows: 0, folded: false, tables: Moved::EVERYTHING };
}

/// The metadata sidecars as this side of the program keeps them.
pub struct Tables {
    held: Sidecars,
    /// Which whole-file tables the directory has no file for at all.
    ///
    /// A table nothing in a run happened to move is a table never written,
    /// and a directory a follower has been filling for an hour can be
    /// missing one that way. To a client that absence is not "nothing to
    /// report": `galos_map` reads a missing supercharge table as "this
    /// index cannot say where a jet cone is" and refuses to plot a route
    /// for a drive that takes one. So the first write of a run writes
    /// whatever the directory lacks, empty if that is what it comes to.
    absent: Moved,
}

impl Tables {
    /// What `dir` already publishes, or empty tables where it publishes
    /// nothing.
    pub fn resume(dir: &Path) -> io::Result<Tables> {
        let (held, absent) = Sidecars::resume(dir)?;
        let counts = held.counts();
        debug!(
            names = counts.names,
            populated = counts.populated,
            reaches = counts.reaches,
            boosts = counts.boosts,
            factions = counts.factions,
            dir = %dir.display(),
            "resumed the metadata tables",
        );
        Ok(Tables { held, absent })
    }

    /// How many systems the names table holds.
    pub fn names(&self) -> usize {
        self.held.counts().names
    }

    /// How many rows each table holds.
    pub fn counts(&self) -> Counts {
        self.held.counts()
    }

    /// Every address the names table holds.
    pub fn named(&self) -> HashSet<i64> {
        self.held.named().collect()
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
            .held
            .named()
            .filter(|address| !drawn.contains(address))
            .collect();
        for address in &orphans {
            self.held.unname(*address);
        }
        orphans.len()
    }

    /// Take what `galaxy` now says about `touched`, answering what moved.
    ///
    /// In memory. The tables that are single files are left for
    /// [`Self::write`], which is what decides between "what moved" and "all
    /// of it", and the per-system body files belong to the galaxy's own store
    /// (`galos_index::bodies`), which is what writes them.
    ///
    /// A system the galaxy has nothing to say about is left exactly as the
    /// directory has it; see the module header.
    pub fn patch(
        &mut self,
        galaxy: &Galaxy,
        touched: &HashSet<i64>,
    ) -> io::Result<Wrote> {
        for &address in touched {
            if let Some(entry) = galaxy.name_of(address) {
                self.held.name(entry);
            }
        }
        let tables = self.patch_tables(galaxy, touched);
        Ok(Wrote { name_rows: 0, folded: false, tables })
    }

    /// Take what `galaxy` says about `touched` into the tables written
    /// whole, leaving the names table alone, and answer what moved.
    ///
    /// What a cold build patches through. That build writes its own names
    /// table straight to disk as it reads — sorted and swapped in at the
    /// end, `galos_index::names::Writer` — so a second copy held here
    /// would be a kilobyte a system over the galaxy, the one thing that
    /// route exists not to hold, and would then be published over the
    /// base the build had just put in place.
    pub fn patch_tables(
        &mut self,
        galaxy: &Galaxy,
        touched: &HashSet<i64>,
    ) -> Moved {
        let mut moved = Moved::default();
        for &address in touched {
            if let Some(said) = galaxy.populated_of(address) {
                let row = over(self.held.published(address), said);
                moved.populated |= self.held.populate(row);
            }

            if let Some(reach) = galaxy.reach_of(address) {
                moved.reaches |= self.held.reach(address, reach);
            }

            if let Some(boost) = galaxy.boost_of(address) {
                moved.boosts |= self.held.boost(address, boost);
            }
        }
        moved
    }

    /// Write the tables `moved` names, the ones the directory has no file
    /// for at all, and whatever the names table has taken.
    ///
    /// The names go to the delta log, which is an append of the changed
    /// rows rather than a rewrite of the table. The rewrite that does fold
    /// them into the base is asked for afterwards — never before, the fold
    /// reading the directory — and comes back in [`Wrote::folded`] because
    /// it is the one part of a publish that costs minutes.
    ///
    /// [`Wrote::EVERYTHING`] writes the lot, which is what a directory being
    /// published from nothing wants.
    pub fn write(&mut self, dir: &Path, moved: Wrote) -> io::Result<Wrote> {
        let mut tables = moved.tables;
        tables.absorb(self.absent);
        let name_rows = self.held.write(dir, tables)?;
        let folded = self.held.compact_names(dir)?;
        self.absent = Moved::default();
        Ok(Wrote { name_rows, folded, tables })
    }
}

/// What an event says about a system, over what the directory publishes.
///
/// A row derived from events is thinner than one derived from the database
/// and always will be: a journal names factions and numbers none of them,
/// so [`galos_index::Galaxy`] publishes an empty faction list by
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
