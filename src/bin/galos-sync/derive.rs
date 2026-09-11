//! The index, kept current from the same readings the database gets.
//!
//! The other half of a run. Where the collect side writes each reading to
//! Postgres as it arrives, this one holds a tree open and publishes what
//! moved on a beat — and it does that on a thread of its own, because a
//! publish is a whole-galaxy `fs::write` and the tables beside it, which is
//! not a thing to do on an executor that is also supposed to be reading a
//! feed at 31 messages a second.
//!
//! ## The handoff
//!
//! With `--db --index` the directory may be missing, or a week behind the
//! database. Ingest must not wait for it, and the index must not take live
//! events until it is level — an index that applied today's scans onto last
//! week's tree would serve a galaxy that is current in the places the feed
//! happened to mention and stale everywhere else, with a resume point
//! claiming otherwise.
//!
//! So: the collect side starts at once and every index-bound reading goes
//! into a bounded channel. This worker runs `galos_db::index::catch_up` —
//! which takes the database clock before it reads, builds or deltas, and
//! writes the directory and a checkpoint whose cursor is that clock — then
//! opens [`Index`] on what the catch-up wrote, drains the channel into it,
//! and goes live.
//!
//! If the channel overflowed while the catch-up ran, the buffer is discarded
//! and another round is run instead of draining it; see [`buffered`]. The
//! rounds converge: the first is a full build of an hour, the second is a
//! few minutes of rows, the third is seconds.
//!
//! Without `--db` there is nothing to catch up from, so the sink opens on
//! whatever the directory already holds and goes live immediately.

use crate::sink::relay::{Dropped, Live, Reading};
use crate::sink::{Index, Sink};
use crate::Shutdown;
use async_channel::Receiver;
use galos_db::index::{self, Parts};
use galos_db::Database;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// How long a quiet channel is waited on before the beat is looked at.
///
/// What this bounds is how late a publish can be when nothing is arriving,
/// and how long Ctrl-C takes to be noticed here. A busy channel never waits:
/// a reading that is ready is taken at once.
const TICK: Duration = Duration::from_millis(100);

/// How long a stopping worker waits for the sources to hand over what they
/// have already read. See [`Derive::last_word`].
const GRACE: Duration = Duration::from_secs(2);

/// What to do with what was buffered while a catch-up round ran.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Buffered {
    /// Apply it to the index and go live.
    Drain,
    /// Throw it away and catch up again.
    CatchUpAgain,
}

/// Which of the two a round that dropped this many readings calls for.
///
/// Nothing dropped: the buffer holds everything that arrived since the
/// process started, the overlap with the catch-up is duplicate work and
/// applying an event twice lands exactly where applying it once did. Drain
/// it and go live.
///
/// Anything dropped: the buffer has a hole in it. Draining it would apply
/// what is there, write a resume point whose cursor says the index is
/// current as of now, and leave every system that fell in the hole stale
/// until something else happens to touch it — which for a system nobody
/// visits is never. So the buffer goes, and another catch-up reads back
/// what was dropped: every one of those readings was written to Postgres by
/// the database sink *before* the relay dropped it, which is the whole
/// reason discarding is safe.
pub fn buffered(dropped: u64) -> Buffered {
    match dropped {
        0 => Buffered::Drain,
        _ => Buffered::CatchUpAgain,
    }
}

/// The index worker: what one thread does for the length of a run.
pub struct Derive {
    pub dir: PathBuf,
    pub checkpoint: PathBuf,
    /// The derive side's own pool, where this run has a database at all.
    pub db: Option<Database>,
    /// How often what has been read is written out.
    pub publish: Duration,
    /// Readings from every source, in the order they were read.
    pub readings: Receiver<Reading>,
    /// Set once this worker is draining, so the relays stop dropping.
    pub live: Live,
    /// What the relays dropped, cleared at the top of each round.
    pub dropped: Dropped,
    pub shutdown: Shutdown,
}

impl Derive {
    /// Catch up, drain, go live, and close the directory out.
    ///
    /// A failure here ends the run rather than only this half of it. The
    /// sources are reading a feed that never stops, and with the worker
    /// gone there is nothing draining what they hand over: the process
    /// would sit there looking busy, writing an index nobody is writing,
    /// and never reach the exit code that says what happened.
    pub async fn run(self) -> Result<(), String> {
        let ran = self.work().await;
        if ran.is_err() {
            self.shutdown.ask();
        }
        ran
    }

    async fn work(&self) -> Result<(), String> {
        let mut sink = self.open().await?;

        // From here nothing may be dropped: this worker is the only reader
        // of the channel and it is reading it now, so a relay that finds it
        // full waits instead of counting.
        self.live.now();
        let drained = self.drain(&mut sink).await;
        if drained > 0 {
            info!(
                readings = drained,
                "what arrived during the catch-up is in the index",
            );
        }

        self.follow(&mut sink).await
    }

    /// An [`Index`] on a directory that is level with the database.
    ///
    /// The rounds of the handoff. Without a database there are none: there
    /// is nothing to be level with and the sink opens on the directory as
    /// it stands.
    async fn open(&self) -> Result<Index, String> {
        let Some(db) = &self.db else {
            return Index::open(&self.dir, &self.checkpoint, None);
        };

        for round in 1.. {
            // Cleared before the round rather than after it, so what is
            // counted is what was dropped while this round ran.
            self.dropped.clear();
            let cursor =
                index::catch_up(db, &self.dir, &self.checkpoint, Parts::ALL)
                    .await
                    .map_err(|err| format!("{err}"))?;
            let dropped = self.dropped.count();
            info!(
                round = round,
                cursor = %cursor,
                dropped = dropped,
                dir = %self.dir.display(),
                "the index is level with the database",
            );

            // Asked to stop mid-handoff: the directory the last round wrote
            // is whole and has a resume point, so this opens on it and the
            // live loop below closes it out rather than another round
            // starting.
            if self.shutdown.asked() {
                break;
            }

            match buffered(dropped) {
                Buffered::Drain => break,
                Buffered::CatchUpAgain => {
                    let thrown = self.discard();
                    warn!(
                        round = round,
                        dropped = dropped,
                        discarded = thrown,
                        "the feed outran the catch-up; what was buffered is \
                         in the database and is being read back rather than \
                         drained with a hole in it",
                    );
                }
            }
        }

        Index::open(&self.dir, &self.checkpoint, self.db.clone())
    }

    /// Everything waiting on the channel right now, applied.
    ///
    /// Not everything that will ever arrive: this is the buffer, and the
    /// sources are still filling it behind this. What it leaves is a channel
    /// that is empty or nearly so, which is where the live loop takes over.
    async fn drain(&self, sink: &mut Index) -> u64 {
        let mut drained = 0;
        while let Ok(reading) = self.readings.try_recv() {
            reading.apply(sink).await;
            drained += 1;
        }
        drained
    }

    /// Throw the buffer away, answering how much of it there was.
    fn discard(&self) -> u64 {
        let mut thrown = 0;
        while self.readings.try_recv().is_ok() {
            thrown += 1;
        }
        thrown
    }

    /// Take readings as they arrive, publishing on the beat.
    ///
    /// Ends when every source has finished — the last sender dropped closes
    /// the channel — or when the run is asked to stop. Either way the
    /// directory is written whole and a resume point with it, which is the
    /// difference between this and a killed process.
    async fn follow(&self, sink: &mut Index) -> Result<(), String> {
        let mut published = Instant::now();
        let mut ended = false;
        while !ended {
            match async_std::future::timeout(TICK, self.readings.recv()).await {
                Ok(Ok(reading)) => reading.apply(sink).await,
                // Every source is done and nothing more is coming.
                Ok(Err(_)) => ended = true,
                Err(_) => {}
            }

            if published.elapsed() >= self.publish {
                if let Err(said) = sink.flush().await {
                    warn!(error = %said, "could not publish");
                }
                published = Instant::now();
            }

            if self.shutdown.asked() {
                self.last_word(sink).await;
                break;
            }
        }

        // What the sources managed to hand over on their way out.
        self.drain(sink).await;

        // The whole directory, not a delta: a run that only ever published
        // what moved leaves behind the tables nothing in it happened to
        // change. This is what the supervisor waits for.
        sink.finish().await?;
        info!("{}", sink.said());
        Ok(())
    }

    /// Everything the sources still have, once the run has been asked to
    /// stop.
    ///
    /// They are stopping too, and each notices at the top of its own loop:
    /// a quarter of a second for the feed, a poll for a followed journal.
    /// Ending here the moment the flag is set would throw away whatever
    /// they read in between — which for a run with no `--db` is gone for
    /// good, since nothing wrote it down. So this keeps draining until the
    /// last sender lets go, which is how a source says it has finished.
    ///
    /// Bounded all the same. `--watch 3600` is a follower that sleeps for
    /// an hour between polls, and a Ctrl-C that took an hour to be obeyed
    /// would be a run nobody could stop.
    async fn last_word(&self, sink: &mut Index) {
        let asked = Instant::now();
        while asked.elapsed() < GRACE {
            match async_std::future::timeout(TICK, self.readings.recv()).await {
                Ok(Ok(reading)) => reading.apply(sink).await,
                // Every source has let go, which is what this waits for.
                Ok(Err(_)) => return,
                Err(_) => {}
            }
        }
    }
}

/// The index brought level with the database, with no events in it at all.
///
/// The `--db --index` run with no `--from`: a rebuild, a repair of one part
/// with `--only`, or a follower of the database with `--watch`. This is what
/// `galos-sync db` was, under the flags that say which way it runs.
pub async fn from_database(
    db: &Database,
    dir: &Path,
    checkpoint: &Path,
    parts: Parts,
    watch: Option<Duration>,
) -> Result<(), String> {
    match watch {
        Some(every) => index::watch(db, dir, checkpoint, every)
            .await
            .map_err(|err| format!("{err}")),
        None => {
            let cursor = index::catch_up(db, dir, checkpoint, parts)
                .await
                .map_err(|err| format!("{err}"))?;
            info!(cursor = %cursor, dir = %dir.display(), "the index is level");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer that kept everything is drained into the index
    ///
    /// The ordinary handoff: a warm directory, a catch-up of a few seconds,
    /// and every reading that arrived while it ran still in the channel.
    /// Applying those twice is what a catch-up overlapping a buffer costs,
    /// and applying an event twice lands exactly where applying it once
    /// did.
    #[test]
    fn a_whole_buffer_is_drained() {
        assert_eq!(buffered(0), Buffered::Drain);
    }

    /// A buffer with a hole in it is thrown away and read back instead
    ///
    /// The case the policy exists for: a cold start, an hour-long build, and
    /// a feed that filled fifty thousand slots long before it finished.
    /// Draining what is left would apply everything except what was dropped
    /// and then write a resume point saying the index is current, which
    /// leaves those systems stale until something touches them again.
    /// Everything discarded was written to Postgres first, so another round
    /// reads it back.
    #[test]
    fn a_buffer_with_a_hole_costs_another_round() {
        assert_eq!(buffered(1), Buffered::CatchUpAgain);
        assert_eq!(buffered(4_000_000), Buffered::CatchUpAgain);
    }
}
