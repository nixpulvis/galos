//! A commander's own journal, served as an index.
//!
//! The map draws from a directory of index files, baked out of Postgres by
//! `galos-sync db` from what EDDN carries. That is everyone else's game, and
//! it is missing exactly one commander's: the systems they have been to that
//! nobody has reported, the bodies only they have scanned, and the jump they
//! made thirty seconds ago. The game already writes all of it, to a directory
//! of `.log` files, on the same machine the map runs on.
//!
//! So this crate reads that directory and answers as a
//! [`galos_index::Source`]. No database, no builder, no published directory:
//! the tree is raised in memory from the journal and rebuilt when the journal
//! moves, and [`galos_index::Layered`] serves it over the published one with
//! a toggle. What has been read can also be written out as a plain index
//! directory — [`JournalSource::publish`], which is what `galos-sync journal
//! --to index=DIR` does — for looking at one commander's own sky on its own.
//!
//! Four modules, in the order the data runs:
//!
//! - [`follow`] — the directory, tailed. Which files, how far into each, and
//!   what to do when one is replaced under you.
//! - [`galaxy`] — the events, accumulated into [`galos_index::System`] and the
//!   metadata records. The peer of `galos_db::index`, and where the three
//!   things a journal cannot say are written down.
//! - [`bodies`] — where what a scan found is kept between one scan and the
//!   next: in memory, or in the index directory's own body files. The one
//!   thing worth choosing about a galaxy, and the difference between a feed
//!   that runs for an hour and one that runs for a week.
//! - [`source`] — the tree over that, rebuilt on change, and the
//!   [`galos_index::Source`] over the tree.
//!
//! Nothing here knows how the galaxy is drawn, and nothing here opens a
//! database.

pub mod bodies;
pub mod follow;
pub mod galaxy;
pub mod source;

pub use bodies::{Bodies, Kept, Published};
pub use follow::{Follower, Read};
pub use galaxy::Galaxy;
pub use source::{JournalSource, Watch};
