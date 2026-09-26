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
//! See `galos_index::accumulate::galaxy`.

use galos_index::accumulate::merge;
use galos_index::store::sidecars::{Counts, Sidecars};
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
    /// A fold rewrites every row the table names — minutes at 200 M systems —
    /// and happens about monthly on the live feed, so it is reported rather
    /// than left silent. See `galos_index::store::names::compact`.
    pub folded: bool,
}

/// The metadata sidecars as this side of the program keeps them, with the
/// tables the program contributes ([`crate::tables`]) beside them.
pub struct Tables {
    held: Sidecars,
}

impl Tables {
    /// What `dir` already publishes, or empty tables where it publishes
    /// nothing.
    ///
    /// A table the directory has no file for at all is written by the
    /// first write of the run, empty if that is what it comes to. A table
    /// nothing in a run happened to move is a table never written, and a
    /// directory a follower has been filling for an hour can be missing one
    /// that way. To a client that absence is not "nothing to report":
    /// `galos_map` reads a missing supercharge table as "this index cannot
    /// say where a jet cone is" and refuses to plot a route for a drive
    /// that takes one.
    pub fn resume(dir: &Path) -> io::Result<Tables> {
        let mut held = Sidecars::resume(dir, &crate::tables())?;
        held.claim_absent();
        let counts = held.counts();
        debug!(
            names = counts.names,
            populated = counts.populated,
            reaches = counts.reaches,
            tables = ?counts.contributed,
            factions = counts.factions,
            dir = %dir.display(),
            "resumed the metadata tables",
        );
        Ok(Tables { held })
    }

    /// How many systems the names table holds.
    pub fn names(&self) -> usize {
        self.held.counts().names
    }

    /// How many rows each table holds.
    pub fn counts(&self) -> Counts {
        self.held.counts()
    }

    /// Whether the names table holds a row for `address`.
    ///
    /// One binary search into the mapping. It was a `HashSet<i64>` of every
    /// address the table held — 5–8 GB transient at 200 M, on a path that
    /// runs at the end of every run — to answer the same question.
    pub fn names_hold(&self, address: i64) -> bool {
        self.held.names_hold(address)
    }

    /// Drop the names of systems `drawn` says the cell tree does not hold,
    /// answering how many went.
    ///
    /// A repair and not an ordinary patch. The two halves of a directory
    /// stand for the same systems or it does not reopen, and a name whose
    /// system is not in the tree is a row the map can find and never draw.
    /// The feed publishes the system again soon enough; the row cannot be
    /// turned back into one.
    ///
    /// `drawn` is asked rather than handed over: the tree can answer for one
    /// address ([`galos_index::Tree::holds`]), and collecting every address
    /// it holds in order to ask is the galaxy in a hash set. What is
    /// collected here is the orphans, which is what the repair is about and
    /// is nothing on a directory that does not need one.
    pub fn forget_names(&mut self, drawn: impl Fn(i64) -> bool) -> usize {
        let orphans: Vec<i64> =
            self.held.named().filter(|address| !drawn(*address)).collect();
        for address in &orphans {
            self.held.unname(*address);
        }
        orphans.len()
    }

    /// Take what `galaxy` now says about `touched`.
    ///
    /// In memory. The tables that are single files are left for
    /// [`Self::write`], and the per-system body files belong to the galaxy's
    /// own store (`galos_index::accumulate::bodies`), which is what writes
    /// them.
    ///
    /// A system the galaxy has nothing to say about is left exactly as the
    /// directory has it; see the module header.
    pub fn patch(&mut self, galaxy: &Galaxy, touched: &HashSet<i64>) {
        for &address in touched {
            if let Some(entry) = galaxy.name_of(address) {
                self.held.name(entry);
            }
        }
        self.patch_tables(galaxy, touched);
    }

    /// Take what `galaxy` says about `touched` into the tables written
    /// whole, leaving the names table alone.
    ///
    /// What a cold build patches through. That build writes its own names
    /// table straight to disk as it reads — sorted and swapped in at the
    /// end, `galos_index::store::names::Writer` — so a second copy held here
    /// would be a kilobyte a system over the galaxy, the one thing that
    /// route exists not to hold, and would then be published over the
    /// base the build had just put in place.
    pub fn patch_tables(&mut self, galaxy: &Galaxy, touched: &HashSet<i64>) {
        for &address in touched {
            if let Some(said) = galaxy.populated_of(address) {
                // What an event says about a system, over what the directory
                // publishes.
                //
                // A row derived from events is thinner than one derived from
                // the database and always will be: a journal names factions and
                // numbers none of them, so `Galaxy` publishes an empty faction
                // list by construction, and the body counts arrive in their own
                // events rather than with the arrival. Writing such a row
                // straight over a published one took the faction ids off every
                // populated system a feed happened to mention — a thousand of
                // them in the directory this was found in — and the map colours
                // and filters by exactly those.
                //
                // So the event wins where it says something and what stands is
                // kept where it does not, which is the rule the database's own
                // write path states column by column. `merge::populated_over`
                // with `newer` set is that rule, stated once in `galos_index`
                // for this and for a merge of two directories.
                let row = match self.held.published(address) {
                    Some(stood) => merge::populated_over(stood, said, true),
                    None => said,
                };
                self.held.populate(row);
            }

            if let Some(reach) = galaxy.reach_of(address) {
                self.held.reach(address, reach);
            }

            // A system nothing says the arrival star of is left as the
            // directory has it, for the reason at the top of this module.
            if let Some(arrival) = galaxy.arrival_of(address) {
                self.held.arrive(&arrival);
            }
        }
    }

    /// Write the tables that moved, the ones the directory has no file for
    /// at all, and whatever the names table has taken.
    ///
    /// The names go to the delta log, which is an append of the changed
    /// rows rather than a rewrite of the table. The rewrite that does fold
    /// them into the base is asked for afterwards — never before, the fold
    /// reading the directory — and comes back in [`Wrote::folded`] because
    /// it is the one part of a publish that costs minutes.
    pub fn write(&mut self, dir: &Path) -> io::Result<Wrote> {
        let name_rows = self.held.write(dir)?;
        let folded = self.held.compact_names(dir)?;
        Ok(Wrote { name_rows, folded })
    }

    /// [`Self::write`], every table whether it moved or not: what a
    /// directory being published from nothing wants.
    pub fn write_everything(&mut self, dir: &Path) -> io::Result<Wrote> {
        self.held.touch_all();
        self.write(dir)
    }
}
