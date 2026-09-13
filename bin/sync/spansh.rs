//! Spansh's `galaxy.json`, a system and everything in it
//!
//! <https://spansh.co.uk/dumps> publishes one complete system per line,
//! bodies nested inside it. The file is tens of gigabytes, so it is read a
//! line at a time through [`spansh::Lines`] and never held whole.
//!
//! Two things reach a sink per line: the system itself, through
//! [`Sink::system`], and one entry per body it gives a home to, through
//! [`Sink::entry`] — which is where a dump's bodies join the scans a journal
//! and EDDN carry.
//!
//! Stamped with the dump's own times — the system with its `date`, each body
//! with its own `updateTime` — and not with `Utc::now()`. The writes
//! downstream are guarded, newer winning and older filling blanks, so a
//! reading claiming to be from now would stand over whatever a commander has
//! sent since the file was published.

use galos::bar;
use galos::sink::{Reporter, Sink, SystemReport};
use galos::{Shard, Shutdown};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::warn;

/// How often the bar's length is estimated again.
const ESTIMATE_EVERY: u64 = 4096;

/// A galaxy dump already on disk: `--from spansh=PATH`.
pub struct Dump {
    pub path: PathBuf,

    /// One line in `n`, where the run was told to take a share of the file.
    pub shard: Option<Shard>,
}

impl Dump {
    /// Read the file, answering whether it could be opened at all.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        let mut lines = match spansh::Lines::open(&self.path) {
            Ok(lines) => lines,
            Err(err) => {
                warn!(
                    file = %self.path.display(),
                    error = %err,
                    "unreadable dump",
                );
                return false;
            }
        };

        let by = format!("Spansh file: {}", self.path.display());
        let tag = match self.shard {
            Some(shard) => format!("Spansh {shard}"),
            None => "Spansh".to_string(),
        };
        let size = std::fs::metadata(&self.path).map(|it| it.len()).ok();
        // How many systems a file holds is not knowable without reading it,
        // so the bar counts systems against a length the mean line seen so
        // far estimates, corrected as it goes. Nothing where the size could
        // not be read, which leaves a bar whose position is a count. A shard
        // counts up to its own share of that estimate and not to the file's.
        let bar = bar::progress(0);
        let (mut read, mut bytes, mut skipped) = (0u64, 0u64, 0u64);
        loop {
            // A dump is a day's read and the run may have been asked to stop
            // an hour into one. What has been written stands: every write is
            // its own guarded upsert and the next run re-reads the file.
            if shutdown.asked() {
                bar.abandon_with_message("stopped");
                break;
            }
            let at = lines.at() + 1;
            let text = match lines.next() {
                Ok(Some(text)) => text,
                Ok(None) => {
                    bar.finish();
                    break;
                }
                // The file stopped being readable part way through, which a
                // half-written dump does. What was read stands.
                Err(err) => {
                    warn!(line = at, error = %err, "dump ended badly");
                    bar.abandon_with_message("unreadable");
                    break;
                }
            };
            // The comma and the newline the reader trimmed off.
            bytes += text.len() as u64 + 2;
            read += 1;
            if let Some(size) = size {
                if read == 1 || read % ESTIMATE_EVERY == 0 {
                    let systems = size / (bytes / read).max(1);
                    bar.set_length(match self.shard {
                        Some(shard) => shard.share(systems),
                        None => systems,
                    });
                }
            }

            // Another process's line, and the cheapest place to find that
            // out: the bytes had to be read to reach the next line, and
            // nothing has been parsed yet. Counted by position in the file,
            // so the shards agree about whose it is without talking to each
            // other.
            if let Some(shard) = self.shard {
                if !shard.mine(read - 1) {
                    continue;
                }
            }
            bar.inc(1);

            // One line nothing can parse is one system missed rather than a
            // run ended: a file this size is not going to be read again for
            // it.
            let system: spansh::galaxy::System =
                match serde_json::from_str(text) {
                    Ok(system) => system,
                    Err(err) => {
                        skipped += 1;
                        warn!(
                            line = at,
                            error = %err,
                            skipped,
                            "unparsed system",
                        );
                        continue;
                    }
                };

            // The scans first, so the name and the place are moved into the
            // report rather than copied out of a system still borrowed.
            let scans = system.scans();
            bar.set_message(format!("[{tag}] {}", system.name));
            sink.system(
                &SystemReport {
                    name: Some(system.name),
                    position: Some(system.coords),
                    population: system.population,
                    security: system.security,
                    government: system.government,
                    allegiance: system.allegiance,
                    primary_economy: system.primary_economy,
                    secondary_economy: system.secondary_economy,
                    body_count: system.body_count,
                    ..SystemReport::new(system.id64, system.update_time)
                },
                &by,
            )
            .await;
            // Nobody flew here and a file was published, so the database
            // takes the file's name as provenance and the index files these
            // bodies under no commander.
            for entry in scans {
                sink.entry(Arc::new(entry), Reporter::Uploader(&by)).await;
            }
        }

        if skipped > 0 {
            warn!(skipped, file = %self.path.display(), "unparsed systems");
        }
        true
    }
}
