//! One process's share of a file
//!
//! A published dump is one file and a whole galaxy, and a sync reading it is
//! waiting on round trips rather than on a processor. Several processes over
//! the same file, each taking a share of it, is the flag that says so:
//! `--shard I/N`. Which is a share of what is decided by the source, this
//! being only the arithmetic and the label.

use std::fmt;

/// One process's share of a file: `--shard I/N`.
///
/// Every `count`th record, offset `index`, so `count` processes over the same
/// file cover it exactly once between them.
///
/// `index < count` and `count >= 1`. The flag's parser is what holds a run to
/// that; a `count` of nought divides by nought.
///
/// What a record is belongs to the source. The line-oriented dumps count
/// records; a journal directory counts files, a file being what says who flew
/// what is in it.
///
/// Every write a shard makes is a guarded upsert keyed by an address, so two
/// shards that cover the same record cost time and nothing else.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Shard {
    /// Which share this process takes, counted from nought.
    pub index: u64,
    /// How many processes are reading the file between them.
    pub count: u64,
}

impl Shard {
    /// Whether the record at `at`, counted from nought, is this one's.
    pub fn mine(&self, at: u64) -> bool {
        at % self.count == self.index
    }

    /// How many of `total` records fall to this shard.
    ///
    /// What a bar counts up to. The remainder goes to the lowest shards, one
    /// apiece, which is where [`mine`](Self::mine) puts it.
    pub fn share(&self, total: u64) -> u64 {
        total / self.count + (total % self.count > self.index) as u64
    }
}

/// Said back the way it was written, which is what a bar is labelled with.
impl fmt::Display for Shard {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}/{}", self.index, self.count)
    }
}
