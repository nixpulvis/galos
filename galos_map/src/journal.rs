//! The commander's own journal, drawn over the published galaxy.
//!
//! The index the map opens is baked from EDDN: everyone else's game. What is
//! missing from it is this commander's — the systems nobody else has
//! reported, the bodies only they have scanned, and the jump they made a
//! minute ago, all of which the game has already written to a directory of
//! `.log` files on this machine. [`galos_journal`] reads that directory and
//! answers the same [`Source`](galos_index::Source) the published index is
//! read through, and [`galos_index::Layered`] serves the one over the other.
//!
//! Set `GALOS_JOURNAL_DIR` and the layer exists; leave it unset and none of
//! this is stood up at all — no resource, no thread, and the transport is the
//! published directory exactly as it was. `J` takes the layer off and puts it
//! back, which costs a re-read of everything the map holds and no restart.
//!
//! ## Why the watch starts late
//!
//! [`galos_index::Claimed`] is how the two layers stay disjoint: the journal
//! leaves out of its own tree every system the published index already
//! carries, keeping all of them in its tables. What answers that question is
//! the names table, which the map reads at startup anyway — two and a half
//! million addresses, already resident, and no second copy of anything.
//!
//! But the map reads that table *through* the layered transport, so it has to
//! be the published index's table and not the joined one. Which it is, so
//! long as the journal has not been read yet: a source nobody has called
//! [`pass`](galos_journal::JournalSource::pass) on holds nothing and adds
//! nothing. So the watch is started here, on the frame the read lands, right
//! after the claim is answered from what was read — and the first pass over
//! the journal is then the first one that could have anything to be disjoint
//! from.
//!
//! The claim is answered from `Names::by_address`, which is the base table as
//! it was read and is never written to afterwards; what arrives later lands in
//! `Names::fresh`, which is where the journal's own names go too. So the claim
//! is stable and answers about the published index alone, which is what it is
//! for. A system the feed publishes *after* startup that this commander has
//! also been to is outside it, and is the one case the two layers both carry:
//! one system counted twice in one cell until the map is next opened.

use crate::loading::Opening;
use crate::{Names, Transport};
use bevy::prelude::*;
use galos_index::{Claimed, Toggle};
use galos_journal::JournalSource;
use galos_journal::source::{EVERY, Watch};
use std::sync::Arc;

/// The environment variable that names the journal directory.
///
/// Beside `GALOS_INDEX_DIR`, and read in the same place: the two are what the
/// map is pointed at, and neither is a setting the map can change about
/// itself while it runs.
pub const DIR: &str = "GALOS_JOURNAL_DIR";

/// The journal layer, where one was asked for.
///
/// Absent as a resource where `GALOS_JOURNAL_DIR` was not set, which is what
/// every reader of it tests: there is no such thing as a map with a journal
/// layer that is not reading a journal.
///
/// Nothing the source can be asked is held here. The diagnostics panel asks
/// [`len`](Self::len) and [`JournalSource::commander`] once a frame while it
/// is open, and each takes the source's lock for read — the same lock the
/// watch thread holds for write across a tail and a rebuild — with the
/// second allocating the commander's name again each time. Holding either
/// here would not save the lock: what says a cached answer is still good is
/// the generation, which is read under that same lock, so the panel would
/// take it as often as it does now and save one short allocation, for state
/// that has to be given up correctly and a `ResMut` on a panel that
/// otherwise writes nothing. The panel is a developer's window and pays
/// only while it is open.
#[derive(Resource)]
pub struct Journal {
    /// The directory being followed, for the diagnostics panel to say.
    pub dir: String,
    /// Whether the layer is being drawn. Shared with the transport, which is
    /// the only thing that acts on it.
    pub on: Toggle,
    /// The source itself, for what it can be asked about its reading.
    pub source: JournalSource,
    /// The thread following the directory, held so that it lives as long as
    /// the map does and stops when the map drops this.
    ///
    /// [`None`] until the names have been read and the claim answered; see
    /// the module header for why it cannot start before that.
    watch: Option<Watch>,
}

impl Journal {
    /// The layer over `dir`, not yet following it.
    pub fn new(dir: String, source: JournalSource, on: Toggle) -> Journal {
        Journal { dir, on, source, watch: None }
    }

    /// How many systems the journal has named so far.
    pub fn len(&self) -> usize {
        self.source.len()
    }

    /// Whether the journal has named nothing at all, which a directory that
    /// has not been read yet also answers.
    ///
    /// Nothing in the map asks it. It stands beside [`len`](Self::len)
    /// because a public length with no emptiness beside it is a thing every
    /// reader has to work out by hand, which is what clippy says about it
    /// too.
    pub fn is_empty(&self) -> bool {
        self.source.is_empty()
    }

    /// Whether the directory is being followed yet.
    pub fn following(&self) -> bool {
        self.watch.is_some()
    }

    /// Take the layer off the sky, or put it back, answering what it now is.
    ///
    /// The whole of the effect is on the transport: with the toggle off the
    /// journal is not asked, and every part the map holds stamps differently,
    /// so the ordinary refresh re-reads the lot and the sky comes back as the
    /// published index alone. See [`galos_index::layer`].
    pub fn flip(&self) -> bool {
        self.on.flip()
    }
}

/// Stand the journal layer's own systems up, where there is a layer.
///
/// The transport itself is built in `main.rs`, beside the published index it
/// is layered over: that is where the map is told what to read, and a
/// transport assembled in two places is a transport nobody can find.
pub fn plugin(app: &mut App) {
    app.add_systems(OnEnter(Opening::Drawn), follow);
}

/// Answer the claim from the names just read, then start following.
///
/// Once, on the frame the index lands. The order is the whole of it: the claim
/// is answered from a table read before the journal had said anything, and the
/// journal is not read until it has been.
fn follow(journal: Option<ResMut<Journal>>, names: Res<Names>) {
    let Some(mut journal) = journal else { return };
    let published = names.by_address.clone();
    let carried = published.len();
    journal
        .source
        .claimed()
        .set(move |address| published.contains_key(&address));
    let watch = journal.source.watch(EVERY);
    journal.watch = Some(watch);
    info!(
        dir = %journal.dir,
        published = carried,
        "following the commander's journal",
    );
}

/// What the map draws from, for `main.rs` to insert.
///
/// The published directory on its own where no journal was named, and the two
/// layered where one was. Named here rather than left inline so that the one
/// place the map decides what it reads through says why in one place.
pub fn transport(
    published: Arc<dyn galos_index::Source>,
    journal: Option<&Journal>,
) -> Transport {
    match journal {
        None => Transport(published),
        Some(journal) => Transport(Arc::new(galos_index::Layered::new(
            published,
            Arc::new(journal.source.clone()),
            journal.on.clone(),
        ))),
    }
}

/// A journal layer over `dir`, claiming nothing yet.
pub fn layer(dir: String) -> Journal {
    let source = JournalSource::over(&dir, Claimed::none());
    Journal::new(dir, source, Toggle::new(true))
}
