//! Importing what the game wrote while it was being played
//!
//! A journal directory read once, in the order it happened, and handed to
//! whatever `--db` and `--index DIR` name -- the database through
//! [`galos_db::record`], or an index directory, which the events reach
//! without going near a row. The same events the EDDN subscriber carries,
//! read from the files rather than off the wire.
//
// TODO: Publishing, which is the direction this does not go yet. Everything
// read here is something EDDN wants and is not getting from this commander,
// and the reading is already done.
//
// What it would take that importing does not is augmentation: a sender has
// to add `StarSystem` and `StarPos` to the events the game writes without
// them, tracked from the last arrival, cross-checked against whatever
// `SystemAddress` the event carries, and the message dropped where the two
// disagree, the game having a habit of pausing its journal and resuming it
// with events missing. Importing is excused that because the row is already
// there to point at; a sender has nobody to point at and has to say the
// whole thing.
//
// Reading a directory the way this does is a better place to do it from than
// a live sender has. The files are whole and in order before anything is
// looked at, so where a sender is guessing from what it has seen so far,
// this can look forward and back and know. What it does not have is the rest
// of what a sender owes EDDN -- the personal fields stripped, the `horizons`
// and `odyssey` flags off `LoadGame`, a schema and a header wrapped around
// each message, and the gateway's rules about how much and how often.

use crate::sink::{Reporter, Sink, SystemName, SystemReport};
use crate::{bar, Shard, Shutdown};
use elite_journal::entry::{Entry, Event, NavRoute};
use elite_journal::journal::{Journal as Reader, Read};
use elite_journal::system::Coordinate;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Where a journal directory remembers whose it is
///
/// A session continued into a second file introduces nobody and
/// `NavRoute.json` never says who is flying, so the name has to come from
/// somewhere. Per directory rather than per run: two directories are as
/// likely to be two commanders as one, and a name carried over from the last
/// import would be a guess about someone else's journal.
const COMMANDER: &str = ".galos-commander";

/// Who a journal is filed under when nothing anywhere says
///
/// Not a commander anyone has, which is the point. These rows came from a
/// journal and that is all this claims about them.
const UNKNOWN: &str = "unknown";

/// How many readings may wait for the side that writes them.
///
/// A reading is whatever the game wrote since the last one, which is a line
/// or a few dozen, so this is minutes of a directory being flown. Full, the
/// watch thread waits rather than dropping: a journal line is the only copy
/// of what it says, where an EDDN message is one of thousands a second and
/// the feed goes on without it.
const WAITING: usize = 64;

/// How long a quiet channel is waited on before the run is asked whether it
/// is still wanted.
///
/// Paid only on the way out and on a directory nobody is flying: it is how
/// long Ctrl-C takes to be noticed while following.
const TICK: Duration = Duration::from_millis(250);

/// A journal directory, or one file in one: `--from journal=PATH`.
pub struct Journal {
    pub path: PathBuf,

    /// Whose journal this is, overriding what the files say. `--user`.
    pub user: Option<String>,

    /// The longest the follow waits on a directory nothing is writing to,
    /// where the run was told to follow one with `--watch`.
    ///
    /// A ceiling and not a poll interval: the filesystem says when a log
    /// was written to, so a jump is read as soon as the game has written
    /// the line whatever this is set to. A second is what the flag defaults
    /// to, which is [`elite_journal::journal::EVERY`].
    pub watch: Option<Duration>,

    /// One file in `n`, where the run was told to take a share of the
    /// directory. `--shard`.
    ///
    /// By file: a file is what names the commander who flew what is in it,
    /// and a shard holding none of the files that name one files its entries
    /// under whatever the directory remembers. `--user` says it for every
    /// shard at once.
    pub shard: Option<Shard>,
    // TODO: `Market.json`, `Shipyard.json` and `Outfitting.json`, which the
    // game keeps beside its logs and rewrites at every station. Nothing reads
    // them yet: `elite_journal` models these three on the shape EDDN sends,
    // which is not the shape the game writes -- camelCase against Pascal,
    // `commodities` against `Items`, and item fields that differ under both.
    // Reading them needs types for the game's shape in `elite_journal`, which
    // convert into the ones `galos_db::record::market` and its neighbors
    // already take.
}

impl Journal {
    /// Import what the path names, and follow it where asked.
    ///
    /// Answers whether all of it could be read. The status is the whole of
    /// what cron reads, so a run that lost a journal to the filesystem must
    /// not look like one with nothing left to do. What could be read is
    /// written either way.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        // Seeded before the import and not after. The offsets are fixed here,
        // so a line the game writes while the import is running is read by
        // both and written twice, which every write downstream is built to
        // survive. Seeded afterwards, that same line would fall in the gap
        // between the two reads and be seen by neither.
        let following = self.watch.map(|every| {
            let dir = if self.path.is_dir() {
                self.path.clone()
            } else {
                self.path.parent().unwrap_or(Path::new(".")).to_owned()
            };
            let reader = Reader::new(&dir);
            match reader.caught_up() {
                Ok(skipped) => debug!(
                    dir = %dir.display(),
                    bytes = skipped,
                    "the import will cover what is already written",
                ),
                Err(err) => warn!(
                    dir = %dir.display(),
                    error = %err,
                    "the directory would not be sized; it will be read twice",
                ),
            }
            (reader, every.max(Duration::from_millis(1)))
        });

        let imported = self.import(sink, shutdown).await;

        if let Some((reader, every)) = following {
            self.follow(sink, reader, every, shutdown).await;
        }

        imported
    }

    /// Read what the game writes, for as long as it writes it.
    ///
    /// Returns only when the run is asked to stop. The filesystem says
    /// which log was written to and a thread inside `elite_journal` turns
    /// that into a reading, so a line reaches the sink as soon as the game
    /// has written it; `every` is only the longest that thread waits over a
    /// directory nobody is flying.
    ///
    /// That thread cannot hold the sink -- its callback is synchronous and
    /// every write here is not -- so what crosses is the reading, over a
    /// bounded channel, the way the EDDN subscriber's thread does it. Full,
    /// the thread waits: a journal line is the only copy of what it says.
    /// Errors reading the directory are the watch's to say and it carries
    /// on past them, so what arrives here is readings only.
    async fn follow(
        &self,
        sink: &mut dyn Sink,
        reader: Reader,
        every: Duration,
        shutdown: &Shutdown,
    ) {
        let (sender, readings) = async_channel::bounded(WAITING);
        let watch = reader.watch(every, move |read| {
            // Closed is the run stopping, which this thread is about to be
            // joined for. There is nowhere left to put a reading.
            let _ = sender.send_blocking(read);
        });
        info!(
            dir = %reader.dir().display(),
            secs = every.as_secs(),
            "following the journal",
        );

        // Who is flying, carried between readings: a session names its
        // commander once, at the top of the file it opened, which may have
        // been read by the import hours ago.
        let mut known = self.user.clone().or_else(|| remembered(reader.dir()));

        while !shutdown.asked() {
            match async_std::future::timeout(TICK, readings.recv()).await {
                Ok(Ok(read)) => {
                    self.followed(sink, reader.dir(), read, &mut known).await
                }
                // The thread has stopped and nothing more is coming.
                Ok(Err(_)) => break,
                Err(_) => {}
            }
        }

        // Closed before the watch is let go of, and not after: dropping a
        // watch joins its thread, and a thread parked in `send_blocking` on
        // a channel nobody is draining is a join with no end to it. Closing
        // first turns that park into an error the thread walks out through.
        readings.close();
        drop(watch);
    }

    /// Write one reading, the way the import writes a directory
    ///
    /// The entries are put in the order they happened, the systems they
    /// name are recorded ahead of anything pointing at one, and then they
    /// are written. That last part is what a live sender cannot do and this
    /// can — the batch is in hand, so the same pre-pass the whole-directory
    /// import runs works over it. What it cannot do is look forward past
    /// the reading, so a signal arriving in one for a system named in the
    /// next is still a write refused, as it is on EDDN.
    async fn followed(
        &self,
        sink: &mut dyn Sink,
        dir: &Path,
        read: Read,
        known: &mut Option<String>,
    ) {
        if read.unread > 0 {
            warn!(lines = read.unread, "entries this cannot read");
        }
        if read.entries.is_empty() {
            return;
        }

        // Shared rather than copied: the same entry is written to Postgres
        // and handed to the index worker.
        let mut entries: Vec<Arc<Entry<Event>>> =
            read.entries.into_iter().map(Arc::new).collect();
        entries.sort_by_key(|entry| entry.timestamp);
        if self.user.is_none() {
            if let Some(name) = commander(&entries) {
                *known = Some(name);
            }
        }
        let user = known.as_deref().unwrap_or(UNKNOWN);

        // The same pre-pass the import runs, over the batch in hand.
        let journals = [(dir.to_owned(), entries)];
        for (address, (_, entry, name, pos)) in gather_names(&journals) {
            sink.system(
                &SystemReport {
                    name: Some(SystemName::new(name)),
                    position: pos,
                    ..SystemReport::new(address, entry.timestamp)
                },
                user,
            )
            .await;
        }

        let [(_, entries)] = &journals;
        for entry in entries {
            sink.entry(Arc::clone(entry), Reporter::Commander(user)).await;
        }
        info!(entries = entries.len(), user = %user, "followed");
    }

    /// Write what the path holds, answering whether all of it could be read
    async fn import(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        let path = self.path.as_path();
        let Ok(meta) = fs::metadata(path) else {
            warn!(path = %path.display(), "nothing to import at this path");
            return false;
        };

        // A directory is a journal directory. A single file is one of the
        // files in one, so the directory holding it is asked who flew this --
        // and is not told anything back. One file is not the directory's, and
        // an archived log imported on its own would otherwise leave its
        // commander behind for everything imported there afterwards.
        let dir = if meta.is_dir() {
            path.to_owned()
        } else {
            path.parent().unwrap_or(Path::new(".")).to_owned()
        };

        let paths = if meta.is_dir() {
            match logs(path) {
                Some(paths) => paths,
                None => return false,
            }
        } else {
            vec![path.to_owned()]
        };

        // This shard's files. Counted by position in the directory, which
        // `logs` orders by name, so the shards agree about whose a file is
        // without talking to each other. Everything below is then the whole
        // of an import of those files: the pre-pass, the bar and the replay
        // all read what is in hand.
        let paths = match self.shard {
            Some(shard) => paths
                .into_iter()
                .enumerate()
                .filter(|(at, _)| shard.mine(*at as u64))
                .map(|(_, path)| path)
                .collect(),
            None => paths,
        };

        // Read whole, as a directory always has been read here. What that
        // buys is order: a write is refused now where something newer already
        // stands in its place, so entries have to reach the database in the
        // order they happened or an import lands differently every time. A
        // file's own first entry is what says where the file belongs, which
        // is the order a commander's name carries forward in.
        let mut journals: Vec<(PathBuf, Vec<Arc<Entry<Event>>>)> = Vec::new();
        let mut refused = 0;
        for path in &paths {
            match read(path) {
                Some(entries) => {
                    // Shared rather than copied, and sorted as pointers
                    // rather than as entries: the same reading is written to
                    // Postgres and handed to the index worker, and an
                    // `Entry<Event>` is a few hundred bytes to move where a
                    // refcount is eight.
                    let mut entries: Vec<Arc<Entry<Event>>> =
                        entries.into_iter().map(Arc::new).collect();
                    entries.sort_by_key(|entry| entry.timestamp);
                    journals.push((path.to_owned(), entries));
                }
                None => refused += 1,
            }
        }
        journals.sort_by_key(|(_, entries)| {
            entries.first().map(|entry| entry.timestamp)
        });

        // What could be read is still worth writing, and re-running costs
        // nothing, so the refused ones do not stop the rest. They do decide
        // the status: a journal not read is a journal not imported, and a run
        // that lost one quietly is a run nobody goes back to.
        if refused > 0 {
            warn!(
                path = %path.display(),
                refused = refused,
                read = journals.len(),
                "journals that could not be read",
            );
        }

        // What the command line said, which outranks every file, or what the
        // directory remembered, which is what a file introducing nobody falls
        // back to.
        let mut known = self.user.clone().or_else(|| remembered(&dir));

        // Who each journal is filed under. A session names its commander at
        // the top of the file it opens and a file continued from it names
        // nobody, so the answer carries forward from the file before.
        let users: Vec<Option<String>> = journals
            .iter()
            .map(|(_, entries)| {
                if self.user.is_none() {
                    if let Some(name) = commander(entries) {
                        known = Some(name);
                    }
                }
                known.clone()
            })
            .collect();

        // The bar's total is entries, since that is what this read gets
        // through; the tally is systems, which the pre-pass below states.
        // Every entry of every log is already in hand here, so the count
        // is known and a byte position is not.
        let mut bar = bar::imported(
            "Journal",
            bar::Extent::Records(
                journals.iter().map(|(_, e)| e.len() as u64).sum(),
            ),
        );

        // Every system the directory names, written before anything points
        // at one. Four of the events the game writes name only an address,
        // and the game writes them ahead of the arrival that would have made
        // the row, so without this the foreign key turns them all away.
        //
        // This is the only place the import states a system, so it is all
        // the bar counts: the entries replayed below write the system they
        // happened in again, which is the same row a second time.
        let names = gather_names(&journals);
        for (address, (journal, entry, name, pos)) in &names {
            let user = users[*journal].as_deref().unwrap_or(UNKNOWN);
            let landed = sink
                .system(
                    &SystemReport {
                        name: Some(SystemName::new(*name)),
                        position: *pos,
                        ..SystemReport::new(*address, entry.timestamp)
                    },
                    user,
                )
                .await;
            bar.took(landed);
        }

        for run in replay(&journals).chunk_by(|(a, _), (b, _)| a == b) {
            let journal = run[0].0;
            let user = users[journal].as_deref().unwrap_or(UNKNOWN);

            for (_, entry) in run {
                // A directory of years is millions of entries, so the run
                // is asked here rather than once a file: what has been
                // written stands, and the next import re-reads the rest.
                if shutdown.asked() {
                    bar.abandoned("stopped");
                    return refused == 0;
                }
                sink.entry(Arc::clone(entry), Reporter::Commander(user)).await;
                bar.through(1);
            }
        }
        bar.done();

        // Whatever is beside the logs, whether one of them or all of them
        // were asked for. The route is where the ship is going now and there
        // is one copy of it, so the directory holding a single file carries
        // it just as much as the directory does. One copy is one shard's:
        // every shard can see the same file, and the first one takes it.
        if self.shard.is_none_or(|shard| shard.index == 0) {
            sidecars(sink, &dir, known.as_deref().unwrap_or(UNKNOWN)).await;
        }

        // Only a whole directory gets to say whose it is, and only what the
        // logs said. A name given on the command line is for the run it was
        // given on, and writing it here would file every later import of
        // this directory under it, the ones that asked for nothing included.
        if meta.is_dir() && self.user.is_none() {
            if let Some(name) = &known {
                remember(&dir, name);
            }
        }

        refused == 0
    }
}

/// Where every system this import names is, by the address it is known by
///
/// The reason an importer reads a whole directory before it writes any of it.
/// The game writes an event per signal as it arrives somewhere and writes the
/// arrival that names the system afterwards, in the same second: 55 of Sol's
/// signals stand above its `FSDJump` in a journal here. So a system's signals
/// reach the database ahead of the thing that would have created the system,
/// and the foreign key turns every one of them away.
///
/// EDDN answers this by making a sender copy a name into every message it
/// forwards. An importer has the whole of it in front of it instead, so it
/// takes the name from wherever in the directory it is given. What is written
/// from this is a name and a place. Everything else about a system is written
/// by the events themselves, in the order they happened.
///
/// The first naming of an address wins, which is the earliest, so the row is
/// stamped at the first the import knows of the place rather than the last.
fn gather_names<'a>(
    journals: &'a [(PathBuf, Vec<Arc<Entry<Event>>>)],
) -> BTreeMap<i64, (usize, &'a Entry<Event>, &'a str, Option<Coordinate>)> {
    let mut names = BTreeMap::new();

    for (journal, entry) in replay(journals) {
        let said: Option<(i64, &str, Option<Coordinate>)> = match &entry.event {
            Event::Location(e) => {
                Some((e.system.address, &e.system.name, e.system.pos))
            }
            Event::CarrierJump(e) => {
                Some((e.system.address, &e.system.name, e.system.pos))
            }
            Event::FsdJump(e) => {
                Some((e.system.address, &e.system.name, e.system.pos))
            }
            Event::Docked(e) => Some((e.system_address, &e.system_name, None)),
            Event::Scan(e) => {
                Some((e.system_address, &e.star_system, e.star_pos))
            }
            Event::ScanBaryCentre(e) => {
                Some((e.system_address, &e.star_system, e.star_pos))
            }
            Event::FssDiscoveryScan(e) => {
                Some((e.system_address, &e.system_name, e.star_pos))
            }
            Event::FssAllBodiesFound(e) => {
                Some((e.system_address, &e.system_name, e.star_pos))
            }
            Event::CodexEntry(e) => {
                Some((e.system_address, &e.system_name, e.star_pos))
            }
            // The events the game writes without a name say nothing here.
            // They are what this is for.
            _ => None,
        };

        if let Some((address, name, pos)) = said {
            names.entry(address).or_insert((
                journal,
                entry.as_ref(),
                name,
                pos,
            ));
        }
    }

    names
}

/// Every entry of every journal, in the order they happened
///
/// Each paired with the journal it came out of, which is what says who flew
/// it and what the bar is showing.
///
/// A whole file at a time is not that order. The Live and Legacy clients
/// write into one Saved Games directory and their files cover the same
/// afternoons, so replaying one file before starting the next puts an older
/// reading of a station over a newer one, which a guarded write cannot tell
/// from an update. Sorting is stable, so entries stamped the same second are
/// left in the order their files stand in.
fn replay(
    journals: &[(PathBuf, Vec<Arc<Entry<Event>>>)],
) -> Vec<(usize, &Arc<Entry<Event>>)> {
    let mut replayed: Vec<_> = journals
        .iter()
        .enumerate()
        .flat_map(|(journal, (_, entries))| {
            entries.iter().map(move |entry| (journal, entry))
        })
        .collect();

    replayed.sort_by_key(|(_, entry)| entry.timestamp);
    replayed
}

/// The journal files in a directory, in the order they were started
///
/// Anything ending `.log`, which is every journal the game has written under
/// either of the two names it has given them. A directory that will not open
/// answers nothing rather than none of them, which is a different thing and
/// is what the import's status turns on.
///
/// By name, which the game makes the order they were started in. `read_dir`
/// answers in whatever order the filesystem holds them, and on a directory
/// of any size that is a hash rather than an order. Two journals whose first
/// entries fall in the same second tie in every sort downstream, and a stable
/// sort breaks a tie by the order it was handed, so leaving this to the
/// filesystem is an import that lands differently between runs.
fn logs(dir: &Path) -> Option<Vec<PathBuf>> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) => {
            warn!(dir = %dir.display(), error = %err, "unreadable directory");
            return None;
        }
    };

    let mut logs: Vec<PathBuf> = read
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("log")
        })
        .collect();

    logs.sort();
    Some(logs)
}

/// Read one journal file, saying what in it could not be read
///
/// A line counted here is one this claims to write, since an event nothing
/// models is read as [`Event::Other`] rather than failing. Said once a file
/// with a count and a reason rather than once a line, a journal that hits
/// this hitting it thousands of times over for the same reason.
fn read(path: &Path) -> Option<Vec<Entry<Event>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            warn!(file = %path.display(), error = %err, "unreadable journal");
            return None;
        }
    };

    entries(BufReader::new(file), path)
}

/// The entries in an open journal, saying what in it could not be read
///
/// Two kinds of failure and only one of them is a line. `InvalidData` is a
/// torn one: `read_until` took its bytes and gave back something that is not
/// UTF-8, so the next line is there to read and this one is counted with the
/// lines that would not parse. Any other error took nothing, and `lines`
/// answers it again every time it is asked, so carrying on past one is a
/// loop with no end to it. That is the file having stopped rather than the
/// line, and it is reported as the file.
fn entries(journal: impl BufRead, path: &Path) -> Option<Vec<Entry<Event>>> {
    let mut found = Vec::new();
    let mut unread = 0;
    let mut why = None;

    for line in journal.lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) if err.kind() == ErrorKind::InvalidData => {
                unread += 1;
                why.get_or_insert_with(|| err.to_string());
                continue;
            }
            Err(err) => {
                warn!(
                    file = %path.display(),
                    error = %err,
                    read = found.len(),
                    "journal stopped being readable",
                );
                return None;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str(&line) {
            Ok(entry) => found.push(entry),
            Err(err) => {
                unread += 1;
                why.get_or_insert_with(|| err.to_string());
            }
        }
    }

    if unread > 0 {
        warn!(
            file = %path.display(),
            unread = unread,
            read = found.len(),
            first = %why.unwrap_or_default(),
            "entries this cannot read",
        );
    }

    Some(found)
}

/// The files the game keeps beside its logs
///
/// Each holds the whole of something as it stands rather than a record of it
/// changing, and is rewritten in place. Only the ones something here reads are
/// looked for; the rest -- `Status.json`, `Cargo.json`, `ShipLocker.json` and
/// their like -- describe a ship and a commander rather than a galaxy, and
/// there is nowhere to put them.
async fn sidecars(sink: &mut dyn Sink, dir: &Path, user: &str) {
    let route = dir.join("NavRoute.json");
    if !route.is_file() {
        return;
    }

    // Read here rather than through `parse_status_file`, which opens the file
    // behind an `unwrap`. This runs after every log has been written, and a
    // route the filesystem would not hand over is no reason to take the whole
    // import down at the end of it.
    let json = match fs::read_to_string(&route) {
        Ok(json) => json,
        Err(err) => {
            warn!(file = %route.display(), error = %err, "unreadable nav route");
            return;
        }
    };

    match serde_json::from_str::<Entry<NavRoute>>(&json) {
        // Through the event path rather than through a call of its own: the
        // game writes a `NavRoute` event into the log beside this file, so a
        // sink already knows what one means and there is nothing here it has
        // to be told separately.
        Ok(entry) => {
            sink.entry(
                Arc::new(Entry {
                    timestamp: entry.timestamp,
                    event: Event::NavRoute(entry.event),
                    horizons: entry.horizons,
                    odyssey: entry.odyssey,
                }),
                Reporter::Commander(user),
            )
            .await;
        }
        Err(err) => {
            warn!(file = %route.display(), error = %err, "unreadable nav route")
        }
    }
}

/// Who flew these entries, where they say
///
/// Named twice at the top of a session, by `Commander` and again by
/// `LoadGame`, and once more by `NewCommander` for the first file a journal
/// ever held. Any of them answers it. A file continued from an earlier session
/// names nobody and is left to whatever the directory remembers.
fn commander(entries: &[Arc<Entry<Event>>]) -> Option<String> {
    entries.iter().find_map(|entry| match &entry.event {
        Event::Commander(commander) => Some(commander.name.clone()),
        Event::NewCommander(new) => Some(new.commander.name.clone()),
        Event::LoadGame(load) => {
            load.commander.as_ref().map(|c| c.name.clone())
        }
        _ => None,
    })
}

/// What a directory was told last time it was imported
fn remembered(dir: &Path) -> Option<String> {
    let name = fs::read_to_string(dir.join(COMMANDER)).ok()?;
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Tell a directory who flew what is in it
///
/// The directory belongs to the game, and writing to it is a courtesy to the
/// next import rather than something this one needs. Refused, say so and carry
/// on: everything is already written.
fn remember(dir: &Path, name: &str) {
    if remembered(dir).as_deref() == Some(name) {
        return;
    }

    let path = dir.join(COMMANDER);
    match fs::write(&path, format!("{}\n", name)) {
        Ok(()) => {
            info!(commander = %name, file = %path.display(), "remembered")
        }
        Err(err) => {
            warn!(file = %path.display(), error = %err, "could not be remembered")
        }
    }
}

/// Reading a journal directory, as far as the first write
///
/// What a file said, who flew it and the order it happened in are settled
/// before a row is touched, so all of it is answerable without a database.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sink::Landed;
    use chrono::{DateTime, Utc};
    use elite_journal::entry::market::{
        BlackMarket, Market, Outfitting, Shipyard,
    };
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn parse(lines: &[&str]) -> Vec<Arc<Entry<Event>>> {
        lines
            .iter()
            .map(|line| {
                Arc::new(
                    serde_json::from_str(line).expect("entry should parse"),
                )
            })
            .collect()
    }

    /// A directory of this test's own, emptied before it writes anything
    ///
    /// Named for the test, so two running at once do not share one, and left
    /// behind afterwards, which is what makes a failure readable.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("galos-journal").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory should be made");
        dir
    }

    /// Write a journal file, byte for byte, and answer where it went
    fn journal(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        File::create(&path)
            .and_then(|mut file| file.write_all(bytes))
            .expect("a journal should be writable");
        path
    }

    /// One line of a journal that is not valid UTF-8
    ///
    /// A torn write or a bad sector. `BufRead::lines` answers this line with
    /// an error and goes on to the next one, so the file does not end here.
    const TORN: &[u8] = b"{\"timestamp\":\"2026-08-08T12:00:01Z\",\
        \"event\":\"Music\",\"MusicTrack\":\"\xff\xfe\"}";

    /// The header every journal file opens with, naming nobody
    const FILEHEADER: &str = r#"{
        "timestamp": "2026-08-08T12:00:00Z",
        "event": "Fileheader",
        "part": 1,
        "language": "English/UK",
        "gameversion": "4.0.0.1904",
        "build": "r308767/r0"
    }"#;

    /// `Commander`, which follows the header at the top of a session
    #[test]
    fn a_journal_names_who_flew_it() {
        let entries = parse(&[
            FILEHEADER,
            r#"{
                "timestamp": "2026-08-08T12:00:01Z",
                "event": "Commander",
                "FID": "F123456",
                "Name": "Nixpulvis"
            }"#,
        ]);

        assert_eq!(commander(&entries).as_deref(), Some("Nixpulvis"));
    }

    /// `LoadGame`, which says the same thing under another key
    #[test]
    fn a_loaded_game_names_one_as_well() {
        let entries = parse(&[
            FILEHEADER,
            r#"{
                "timestamp": "2026-08-08T12:00:02Z",
                "event": "LoadGame",
                "FID": "F123456",
                "Commander": "Nixpulvis",
                "Horizons": true,
                "GameMode": "Solo",
                "Credits": 1000,
                "Loan": 0
            }"#,
        ]);

        assert_eq!(commander(&entries).as_deref(), Some("Nixpulvis"));
    }

    /// A session continued into a second file introduces nobody
    ///
    /// The case the directory is asked to remember for. Nothing in the file
    /// says who flew it, and the entries in it are worth exactly as much as
    /// the ones in the file it continues.
    #[test]
    fn a_continued_journal_names_nobody() {
        let entries = parse(&[
            FILEHEADER,
            r#"{
                "timestamp": "2026-08-08T12:00:03Z",
                "event": "FSDJump",
                "StarSystem": "Sol",
                "StarPos": [0.0, 0.0, 0.0],
                "SystemAddress": 10477373803
            }"#,
        ]);

        assert_eq!(commander(&entries), None);
    }

    /// A line the filesystem will not hand over costs that line and no more
    ///
    /// One bad byte in a file of thousands. Everything either side of it was
    /// written by the game and is worth as much as it ever was.
    #[test]
    fn a_torn_line_does_not_take_the_file_with_it() {
        let dir = scratch("a_torn_line");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            br#"{"timestamp":"2026-08-08T12:00:00Z","event":"NavRoute"}"#,
        );
        bytes.push(b'\n');
        bytes.extend_from_slice(TORN);
        bytes.push(b'\n');
        bytes.extend_from_slice(
            br#"{"timestamp":"2026-08-08T12:00:02Z","event":"NavRoute"}"#,
        );
        bytes.push(b'\n');

        let entries = read(&journal(&dir, "Journal.torn.log", &bytes))
            .expect("the file should read");

        assert_eq!(entries.len(), 2);
    }

    /// A journal that stops being readable is not a journal read to the end
    ///
    /// `BufRead::lines` answers an error that consumed nothing by answering
    /// it again, and again, every time it is asked. So counting one and
    /// carrying on never reaches the end of the file: the import hangs, and
    /// says nothing while it does. Only a torn line is worth carrying on
    /// past, and a torn line is the one whose bytes were taken.
    #[test]
    fn a_journal_that_stops_being_readable_is_not_read_to_the_end() {
        /// A reader that fails the way a disk going away fails
        struct Failing(usize);

        impl std::io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0 += 1;
                // Ends the file rather than the test, so a reader that
                // carries on past the error fails on the answer below
                // instead of hanging the suite.
                if self.0 > 100 {
                    return Ok(0);
                }
                Err(std::io::Error::other("the disk went away"))
            }
        }

        let dying = BufReader::new(Failing(0));

        assert!(entries(dying, Path::new("Journal.dying.log")).is_none());
    }

    /// A directory that will not open is not one holding no journals
    ///
    /// They look alike from here, and the import's status turns on telling
    /// them apart: nothing to import is a run that worked, and a directory
    /// the filesystem refused is not.
    #[test]
    fn a_directory_that_will_not_open_answers_nothing() {
        let dir = scratch("a_directory_that_will_not_open");

        assert_eq!(logs(&dir), Some(Vec::new()));
        assert_eq!(logs(&dir.join("no-such-directory")), None);
    }

    /// The journals of a directory come back in the order they were written
    ///
    /// `read_dir` answers in whatever order the filesystem holds them, which
    /// is not an order and need not be the same one twice. Two journals whose
    /// first entries fall in the same second tie in every sort downstream,
    /// and a tie is broken by the order they arrived in, so leaving that to
    /// the filesystem is an import that lands differently between runs. The
    /// game names them for when they were started, so the name is the order.
    #[test]
    fn the_journals_of_a_directory_are_in_the_order_they_were_started() {
        let dir = scratch("the_journals_of_a_directory");

        // Enough of them, written in the reverse of the order wanted, that a
        // directory held by hash has room to disagree with both. Three would
        // come back in order on a filesystem that happened to oblige and
        // pass this whatever the code did.
        let mut wanted: Vec<String> = (3..28)
            .map(|day| format!("Journal.2026-08-{:02}T000000.01.log", day))
            .collect();
        for name in wanted.iter().rev() {
            journal(&dir, name, b"");
        }
        journal(&dir, "NavRoute.json", b"{}");
        wanted.sort();

        let found: Vec<String> = logs(&dir)
            .expect("the directory should read")
            .iter()
            .map(|path| {
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect();

        assert_eq!(found, wanted);
    }

    /// A journal that will not open is not one holding no entries
    #[test]
    fn a_journal_that_will_not_open_answers_nothing() {
        let dir = scratch("a_journal_that_will_not_open");

        let empty = read(&journal(&dir, "Journal.empty.log", b""));
        assert_eq!(empty.map(|entries| entries.len()), Some(0));
        assert!(read(&dir.join("no-such-journal.log")).is_none());
    }

    /// An entry that is nothing but the moment it happened
    fn at(minute: &str) -> Arc<Entry<Event>> {
        Arc::new(
            serde_json::from_str(&format!(
                r#"{{ "timestamp": "2026-08-08T{}:00Z", "event": "NavRoute" }}"#,
                minute,
            ))
            .expect("entry should parse"),
        )
    }

    /// The minute each entry of a replay happened, in the order it is written
    fn minutes(journals: &[(PathBuf, Vec<Arc<Entry<Event>>>)]) -> Vec<String> {
        replay(journals)
            .iter()
            .map(|(_, entry)| entry.timestamp.format("%H:%M").to_string())
            .collect()
    }

    /// A file at a time is not the order the game wrote them in
    ///
    /// The Live and Legacy clients write into one Saved Games directory, so
    /// two files covering the same afternoon is an ordinary directory rather
    /// than a damaged one.
    #[test]
    fn overlapping_journals_are_replayed_in_the_order_they_happened() {
        let journals = vec![
            (PathBuf::from("Journal.live.log"), vec![at("12:00"), at("18:00")]),
            (PathBuf::from("Journal.legacy.log"), vec![at("12:30")]),
        ];

        assert_eq!(minutes(&journals), ["12:00", "12:30", "18:00"]);
    }

    /// A system is named by an event standing below what points at it
    ///
    /// What the game writes on arriving somewhere: the signals it finds
    /// first, then the jump saying where it got to, all in one second. The
    /// signals name only an address, so read a line at a time there is
    /// nothing to make the row from and the foreign key turns every one of
    /// them away. Reading the directory whole is what answers it.
    #[test]
    fn a_system_named_below_what_points_at_it_is_still_named() {
        let journals = vec![(
            PathBuf::from("Journal.arriving.log"),
            parse(&[
                r#"{
                    "timestamp": "2026-08-12T04:02:36Z",
                    "event": "FSSSignalDiscovered",
                    "SystemAddress": 10477373803,
                    "SignalName": "Titan City"
                }"#,
                r#"{
                    "timestamp": "2026-08-12T04:02:36Z",
                    "event": "FSDJump",
                    "StarSystem": "Sol",
                    "StarPos": [1.0, 2.0, 3.0],
                    "SystemAddress": 10477373803
                }"#,
            ]),
        )];

        let names = gather_names(&journals);
        let (_, _, name, pos) =
            names.get(&10477373803).expect("Sol should be named");

        assert_eq!(*name, "Sol");
        assert_eq!(pos.map(|place| place.x), Some(1.0));
    }

    /// Entries stamped the same second keep the order their files are in
    ///
    /// The journal is stamped to the second and a busy one writes several
    /// inside one, so this decides more than a corner case. Sorting is
    /// stable, so what settles it is where the files stand.
    #[test]
    fn entries_stamped_alike_are_left_as_they_stand() {
        let journals = vec![
            (PathBuf::from("Journal.first.log"), vec![at("12:00")]),
            (PathBuf::from("Journal.second.log"), vec![at("12:00")]),
        ];

        let replayed = replay(&journals);
        assert_eq!(
            replayed.iter().map(|(j, _)| *j).collect::<Vec<_>>(),
            [0, 1]
        );
    }

    /// A sink that counts the entries reaching it, readable from elsewhere
    ///
    /// The follow holds the sink for as long as it runs, so a test watching
    /// for its own line to arrive cannot ask the sink itself.
    struct Counted(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl Sink for Counted {
        async fn entry(
            &mut self,
            _: Arc<Entry<Event>>,
            _: Reporter<'_>,
        ) -> Option<Landed> {
            self.0.fetch_add(1, Ordering::Relaxed);
            None
        }
        async fn system(
            &mut self,
            _: &SystemReport,
            _: &str,
        ) -> Option<Landed> {
            None
        }
        async fn market(&mut self, _: DateTime<Utc>, _: &str, _: &Market) {}
        async fn outfitting(
            &mut self,
            _: DateTime<Utc>,
            _: &str,
            _: &Outfitting,
        ) {
        }
        async fn shipyard(&mut self, _: DateTime<Utc>, _: &str, _: &Shipyard) {}
        async fn black_market(
            &mut self,
            _: DateTime<Utc>,
            _: &str,
            _: &BlackMarket,
        ) {
        }
        async fn flush(&mut self) -> Result<(), String> {
            Ok(())
        }
        async fn finish(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn said(&self) -> String {
            String::new()
        }
    }

    /// Wait for `count` entries to have been written, answering whether they
    /// were. Bounded, so a line that never arrives fails a test rather than
    /// hanging the suite.
    fn until(seen: &AtomicUsize, count: usize, within: Duration) -> bool {
        let waited = std::time::Instant::now();
        while seen.load(Ordering::Relaxed) < count {
            if waited.elapsed() > within {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// One line of a journal, compact, as the game writes them
    const HEADER: &[u8] = br#"{"timestamp":"2026-08-08T12:00:00Z","event":"Fileheader","part":1,"language":"English/UK","gameversion":"4.0.0.1904","build":"r308767/r0"}"#;
    const JUMP: &[u8] = br#"{"timestamp":"2026-08-08T12:05:00Z","event":"FSDJump","StarSystem":"Sol","StarPos":[0.0,0.0,0.0],"SystemAddress":10477373803}"#;

    /// A line written while following does not wait for the beat
    ///
    /// What the watch is for, and the only thing that tells it from the
    /// timer it replaced: the beat is set to a minute, so a line written
    /// after the import and read a moment later was read because the
    /// filesystem said so. A reader on this beat would still be asleep.
    ///
    /// Ctrl-C is the other half of it. Asking the run to stop has to end the
    /// follow, which means the watch thread joined and the channel closed,
    /// so a test that hangs here is one this would not return from.
    #[test]
    fn a_line_written_while_following_does_not_wait_for_the_beat() {
        let dir = scratch("a_line_written_while_following");
        let log = journal(&dir, "Journal.watched.log", b"");
        fs::write(&log, [HEADER, b"\n"].concat())
            .expect("a journal should be writable");

        let seen = Arc::new(AtomicUsize::new(0));
        let shutdown = Shutdown::new();
        let mut sink = Counted(Arc::clone(&seen));

        let writing = {
            let (seen, shutdown) = (Arc::clone(&seen), shutdown.clone());
            std::thread::spawn(move || {
                // The import's own entry says the follow is what is left,
                // and a moment more says the watch is up. A line written
                // before that would be read by the import instead and prove
                // nothing about the watch.
                let imported = until(&seen, 1, Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(500));

                let wrote = std::time::Instant::now();
                fs::OpenOptions::new()
                    .append(true)
                    .open(&log)
                    .and_then(|mut file| {
                        file.write_all(&[JUMP, b"\n"].concat())
                    })
                    .expect("the journal should take another line");

                let arrived = until(&seen, 2, Duration::from_secs(10))
                    .then(|| wrote.elapsed());
                shutdown.ask();
                (imported, arrived)
            })
        };

        // Long enough that nothing here can be the timer coming round.
        let beat = Duration::from_secs(60);
        let source =
            Journal { path: dir, user: None, watch: Some(beat), shard: None };
        async_std::task::block_on(source.read(&mut sink, &shutdown));

        let (imported, arrived) =
            writing.join().expect("the writer should finish");
        assert!(imported, "the import should have written its entry");
        let arrived = arrived.expect("the line should have arrived");
        assert!(
            arrived < Duration::from_secs(5),
            "the line took {:?} to arrive",
            arrived,
        );
    }
}
