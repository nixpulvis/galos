//! Reading a publisher of Elite's galaxy, whatever is being written to.
//!
//! A source knows how to read one publisher — journal files, the EDDN feed,
//! an EDSM dump or API, a saved EDDB dump, a Spansh galaxy dump — and says
//! what it read through [`crate::sink::Sink`]. A sink knows what that means
//! on its side: rows in Postgres, or a `galos_index` directory a client
//! draws from with no server at all.
//!
//! **Here rather than in a binary because there are two of them.**
//! `galos-index ingest` and `galos-db ingest` read the same publishers into
//! different sinks; the reading is the same sentence either way, and the
//! only thing that differs is what [`collect`] is handed to fan into. That
//! was one program with a `--db`/`--index` pair of flags and the dozen
//! refusals it took to say which combinations meant anything; it is two
//! programs and no flags now.
//!
//! ```text
//! eddn ────────┐
//! journal=PATH ┼─> events ─┬─> galos-db     (Postgres, per message)
//! edsm=PATH ───┘           └─> galos-index  (cell tree + tables, on a beat)
//! ```
//!
//! Events are the live input to both. A database is what an index is
//! rebuilt *from*, not what it is maintained from, so a running ingest
//! never reads the database to keep a directory current — see
//! [`derive`] for the one path that does, which is a build rather than
//! an ingest.
//!
//! The sinks are not interchangeable and are not meant to be. A database
//! keeps stations, markets, signals and factions; an index keeps the sky
//! and what is inside a system. A source hands over everything it read and
//! each sink takes what it is for. See [`crate::sink`].

use crate::sink::{Fan, Sink};
use crate::{Shard, Shutdown};
use from::Source;
use std::time::Duration;
use tracing::{error, info, warn};

pub mod cold;
pub mod derive;
mod eddb;
pub mod eddn;
mod edsm;
pub mod from;
mod journal;
pub mod spansh;

/// How often an index is asked to write what it has taken, where the run is
/// following something.
///
/// A database has written each message as it arrived and does nothing on a
/// beat. An index has been editing a tree in memory, and this is how often
/// that reaches the disk: a publish rewrites the index file whole, so doing
/// it per message at thirty a second would be thirty rewrites a second to
/// move one system. `--publish` says otherwise.
///
/// A run whose sources all end has no beat at all: there is nothing to see
/// a half-written directory and it is written whole when the run finishes.
pub const PUBLISH_EVERY: u64 = 5;

/// Everything the per-source flags say, carried to whichever source wants
/// them.
///
/// One struct rather than seven arguments: the sources run concurrently, so
/// each task needs its own copy of what it was told, and a command line is
/// not something to clone per source.
#[derive(Clone)]
pub struct Options {
    /// Who to file readings under, where the source names nobody.
    pub user: Option<String>,
    /// The feed's address, where it is not the published one.
    pub remote: Option<String>,
    /// How long a silent feed connection is left alone before it is
    /// reopened; [`None`] leaves it alone however quiet it goes.
    pub stall: Option<Duration>,
    /// Keep reading after the end of what is there.
    pub watch: Option<Duration>,
    /// EDSM API: the cube, in light years, to ask about around a system.
    pub cube: Option<u32>,
    /// EDSM API: the sphere, in light years, to ask about instead.
    pub sphere: Option<u32>,
    /// One process's share of a file, from `--shard`.
    pub shard: Option<Shard>,
    /// What this tool calls itself in a spool's `cursors/` directory. One
    /// name per consumer, so the two tools keep their own places in the
    /// same spool and neither can move the other's.
    pub consumer: &'static str,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            user: None,
            remote: None,
            stall: None,
            watch: None,
            cube: None,
            sphere: None,
            shard: None,
            consumer: from::CONSUMER,
        }
    }
}

/// The per-source flags of an ingest, exactly as the command line gave
/// them.
///
/// **Both tools' `ingest` takes these same eight flags, so the rules about
/// them are written once, here.** Each is a flag that qualifies a reading
/// rather than naming one, and each is refused where the run reads no
/// source it could qualify — a feed's address given to a run that reads a
/// dump, a commander named over EDDN. Two copies of those rules drift: a
/// tenth source, or one more flag, and the two tools disagree about what
/// a command line means.
///
/// Raw rather than an [`Options`], because a refusal has to tell "unset"
/// from "set to the value that happens to be the default" — `--stall 0`
/// and no `--stall` at all both come out as "leave the connection alone"
/// once [`Self::options`] has folded them together.
pub struct Qualifiers<'a> {
    pub sources: &'a [Source],
    pub user: Option<&'a str>,
    pub remote: Option<&'a str>,
    pub stall: Option<u64>,
    pub watch: Option<u64>,
    pub cube: Option<u32>,
    pub sphere: Option<u32>,
    pub shard: Option<Shard>,
}

impl Qualifiers<'_> {
    /// Whether this run follows something rather than reading it out.
    ///
    /// What the index's publish beat turns on, and what `--watch` is
    /// refused for the absence of. See [`Source::follows`].
    pub fn following(&self) -> bool {
        let watching = self.watch.is_some();
        self.sources.iter().any(|source| source.follows(watching))
    }

    /// What the sources are handed, with `consumer` as this tool's name in
    /// a spool.
    ///
    /// `--stall` is the only one that is not a copy, because three states
    /// are spelled with two: nothing said is the default window, `0` is a
    /// connection left alone however quiet it goes, and [`None`] in the
    /// answer is that second one rather than "unset".
    ///
    /// `--remote` is handed over as it was given. Where the feed's own
    /// address is the default, [`collect`] fills it in, so there is one
    /// place the published address is written down.
    pub fn options(&self, consumer: &'static str) -> Options {
        Options {
            user: self.user.map(str::to_owned),
            remote: self.remote.map(str::to_owned),
            stall: match self.stall {
                None => Some(eddn::STALL),
                Some(0) => None,
                Some(secs) => Some(Duration::from_secs(secs)),
            },
            watch: self.watch.map(Duration::from_secs),
            cube: self.cube,
            sphere: self.sphere,
            shard: self.shard,
            consumer,
        }
    }

    /// Refuse the combinations that cannot mean anything.
    ///
    /// Each of these is a run that would otherwise start, do something
    /// other than what was asked, and say nothing about it — which is
    /// worse than not starting, since what these write is a database and a
    /// directory a map is served from.
    ///
    /// Two refusals the flags would seem to need are not here, being said
    /// earlier and better. `--shard 8/8` and `--shard 0/0` are refused as
    /// the value is parsed, by [`from::shard`], which can name the range
    /// it wanted; and a run with no source cannot be spelled, `--from`
    /// being required in both tools.
    ///
    /// What is *not* here is anything about which store is being written:
    /// that used to be most of it, and it went with the flags that said
    /// it. See `bin/index/fill.rs` and `bin/db/ingest.rs` for the two
    /// rules that are one tool's own.
    pub fn refused(&self) -> Result<(), String> {
        for (at, source) in self.sources.iter().enumerate() {
            if self.sources[..at].contains(source) {
                return Err(format!(
                    "--from {source} was named twice, and one run cannot \
                     read it as two sources"
                ));
            }
        }

        if self.shard.is_some() {
            if self.watch.is_some() {
                return Err("--shard divides a file that is all there; \
                            --watch is a feed with no end to divide"
                    .to_string());
            }
            for source in self.sources {
                if let Source::Eddn | Source::EdsmApi(_) = source {
                    return Err(format!(
                        "--shard divides a file between processes and \
                         `{source}` is not a file"
                    ));
                }
            }
        }

        let journal =
            self.sources.iter().any(|it| matches!(it, Source::Journal(_)));
        let api =
            self.sources.iter().any(|it| matches!(it, Source::EdsmApi(_)));
        let feed = self.sources.contains(&Source::Eddn);

        if self.watch.is_some() && !self.following() {
            return Err("--watch follows what is still being written and \
                        nothing here is: the feed, a spool, or a journal \
                        directory. A dump is read out and the run ends"
                .to_string());
        }

        if self.user.is_some() && !journal {
            return Err("--user is whose journal is being read and this run \
                        reads none: --from journal=PATH"
                .to_string());
        }

        if self.remote.is_some() && !feed {
            return Err("--remote is where the feed is subscribed to and \
                        this run does not read it: --from eddn"
                .to_string());
        }

        if self.stall.is_some() && !feed {
            return Err("--stall is how long the feed may carry nothing \
                        before its connection is replaced and this run \
                        does not read it: --from eddn"
                .to_string());
        }

        if self.cube.is_some() && !api {
            return Err("--cube is how much of a neighbourhood EDSM's API \
                        is asked for and this run does not ask it: --from \
                        edsm-api=NAME"
                .to_string());
        }

        if self.sphere.is_some() && !api {
            return Err("--sphere is how much of a neighbourhood EDSM's API \
                        is asked for and this run does not ask it: --from \
                        edsm-api=NAME"
                .to_string());
        }

        // A file named on the command line and not there is a typo, and a
        // run that carried on would open what it writes to, say nothing,
        // and exit having written nothing. The run that found this wrote
        // an empty index over a directory that had a galaxy in it.
        for source in self.sources {
            let named = match source {
                Source::Journal(path)
                | Source::Edsm(path)
                | Source::Eddb(path)
                | Source::Spool(path, _)
                | Source::Spansh(path) => path,
                Source::Eddn | Source::EdsmApi(_) => continue,
            };
            if !named.exists() {
                return Err(format!(
                    "--from {source} is not there. A leading `~` is \
                     expanded here, so a path that still cannot be found \
                     is the path"
                ));
            }
        }

        Ok(())
    }
}

/// Read one source into its own sinks, and close them out.
///
/// One of these per `--from`, run at once. Each has a sink of its own — the
/// pool or channel behind it is shared — so a dump being read does not wait
/// on the feed, and the feed does not wait on it.
pub async fn collect(
    source: Source,
    options: Options,
    mut fan: Fan,
    shutdown: Shutdown,
) -> bool {
    let read = match source {
        Source::Eddn => {
            let remote =
                options.remote.unwrap_or_else(|| eddn::URL.to_string());
            eddn::Eddn::live(&remote, options.stall)
                .read(&mut fan, &shutdown)
                .await
        }
        Source::Spool(dir, start) => {
            match eddn::Eddn::spooled(&dir, start, options.consumer) {
                Ok(spool) => spool.read(&mut fan, &shutdown).await,
                Err(err) => {
                    error!(error = %err, "the spool could not be opened");
                    false
                }
            }
        }
        Source::Journal(path) => {
            let journal = journal::Journal {
                path,
                user: options.user,
                watch: options.watch,
                shard: options.shard,
            };
            journal.read(&mut fan, &shutdown).await
        }
        Source::Edsm(path) => {
            edsm::Dump { path, shard: options.shard }
                .read(&mut fan, &shutdown)
                .await
        }
        Source::EdsmApi(name) => {
            let api =
                edsm::Api { name, cube: options.cube, sphere: options.sphere };
            api.read(&mut fan, &shutdown).await
        }
        Source::Eddb(path) => {
            eddb::Eddb { path, shard: options.shard }
                .read(&mut fan, &shutdown)
                .await
        }
        Source::Spansh(path) => {
            spansh::Dump { path, shard: options.shard }
                .read(&mut fan, &shutdown)
                .await
        }
    };

    // Each sink decides what finishing means for it: nothing for a database,
    // which wrote every message as it arrived, and a last word for a relay,
    // which has handed everything on to a worker that closes the directory
    // out itself.
    let closed = match fan.finish().await {
        Ok(()) => true,
        Err(said) => {
            warn!(error = %said, "could not close the sinks out");
            false
        }
    };
    for said in fan.said_each() {
        info!("{said}");
    }
    read && closed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The flags of an ingest, with nothing named but the sources.
    fn reading(sources: &[Source]) -> Qualifiers<'_> {
        Qualifiers {
            sources,
            user: None,
            remote: None,
            stall: None,
            watch: None,
            cube: None,
            sphere: None,
            shard: None,
        }
    }

    fn journal() -> Source {
        Source::Journal(PathBuf::from("bin"))
    }

    fn dump() -> Source {
        Source::Spansh(PathBuf::from("Cargo.toml"))
    }

    /// A file that is not there is refused before anything is opened
    ///
    /// The run that found this wrote an empty index over a directory that
    /// had a galaxy in it: the dump could not be opened, the source read
    /// nothing, and the worker published the nothing it was handed. So the
    /// check is here, where a refusal costs nothing, and not in the
    /// source.
    #[test]
    fn a_source_that_is_not_there_is_refused() {
        let missing = [Source::Spansh(PathBuf::from("/no/such/galaxy.json"))];
        let Err(said) = reading(&missing).refused() else {
            panic!("a dump that does not exist was accepted")
        };
        assert!(
            said.contains("galaxy.json") && said.contains("not there"),
            "should name the path and what is wrong: {said}",
        );

        // The feed and the API name no file, so neither is checked for
        // one; a directory that is there is a source, journals being
        // directories.
        for sources in [
            vec![Source::Eddn],
            vec![Source::EdsmApi("Sol".to_owned())],
            vec![journal()],
        ] {
            assert!(
                reading(&sources).refused().is_ok(),
                "{sources:?} names nothing missing",
            );
        }
    }

    /// The same source twice is one publisher read twice over
    ///
    /// Over EDDN that is two subscriptions carrying the same galaxy and
    /// every message written twice; over a dump it is the same file read
    /// into the same rows. Two *different* journals are the point of the
    /// flag repeating and are let through.
    #[test]
    fn the_same_source_cannot_be_named_twice() {
        let twice = [Source::Eddn, Source::Eddn];
        let Err(said) = reading(&twice).refused() else {
            panic!("one feed named twice was accepted")
        };
        assert!(said.contains("twice"), "should say why: {said}");

        let two = [journal(), Source::Journal(PathBuf::from("src"))];
        assert!(
            reading(&two).refused().is_ok(),
            "two journals are two directories, not one read twice",
        );
    }

    /// A share is a share of a file, so only the sources that read one
    ///
    /// `eddn` is a subscription and `edsm-api` is an answer about one
    /// system's neighbourhood. Neither divides, and a run that took the
    /// flag anyway would be N processes each writing the whole of the same
    /// feed.
    #[test]
    fn a_share_belongs_to_a_source_that_reads_a_file() {
        let share = Some(Shard { index: 0, count: 8 });
        for source in [dump(), journal(), Source::Eddb(PathBuf::from("src"))] {
            let sources = [source.clone()];
            let it = Qualifiers { shard: share, ..reading(&sources) };
            assert!(it.refused().is_ok(), "{source:?} reads a file");
        }

        for source in [Source::Eddn, Source::EdsmApi("Sol".to_owned())] {
            let sources = [source.clone()];
            let it = Qualifiers { shard: share, ..reading(&sources) };
            let Err(said) = it.refused() else {
                panic!("a sharded {source:?} was accepted")
            };
            assert!(said.contains("--shard"), "should say why: {said}");
        }

        // A followed journal is not a file either: the share is of what is
        // there when the run starts, and what arrives after it is every
        // share's.
        let sources = [journal()];
        let followed =
            Qualifiers { shard: share, watch: Some(1), ..reading(&sources) };
        assert!(followed.refused().is_err(), "a sharded watch was accepted",);
    }

    /// `--watch` follows what is still being written, so there must be one
    ///
    /// A dump is all there when the run starts: a run told to watch one
    /// would read it out and exit anyway, which is the flag doing nothing.
    #[test]
    fn a_watch_belongs_to_something_still_being_written() {
        let sources = [dump()];
        let Err(said) =
            Qualifiers { watch: Some(1), ..reading(&sources) }.refused()
        else {
            panic!("a watched dump was accepted")
        };
        assert!(said.contains("--watch"), "should say why: {said}");

        for sources in [vec![journal()], vec![Source::Eddn]] {
            let it = Qualifiers { watch: Some(1), ..reading(&sources) };
            assert!(it.refused().is_ok(), "{sources:?} is still being written");
        }
    }

    /// The feed's flags belong to a run that reads the feed
    #[test]
    fn the_feeds_flags_need_the_feed() {
        let sources = [dump()];
        let Err(said) =
            Qualifiers { remote: Some("tcp://x:1"), ..reading(&sources) }
                .refused()
        else {
            panic!("an address for a feed nothing reads was accepted")
        };
        assert!(said.contains("--remote"), "should say why: {said}");

        let sources = [journal()];
        let Err(said) =
            Qualifiers { stall: Some(30), ..reading(&sources) }.refused()
        else {
            panic!("a stall window for a feed nothing reads was accepted")
        };
        assert!(said.contains("--stall"), "should say why: {said}");

        let sources = [Source::Eddn];
        let both = Qualifiers {
            remote: Some("tcp://x:1"),
            stall: Some(30),
            ..reading(&sources)
        };
        assert!(both.refused().is_ok());
    }

    /// A neighbourhood is what EDSM's API is asked for, so a run must ask
    /// it
    #[test]
    fn a_neighbourhood_needs_the_api_to_ask() {
        let sources = [Source::Edsm(PathBuf::from("Cargo.lock"))];
        let Err(said) =
            Qualifiers { cube: Some(50), ..reading(&sources) }.refused()
        else {
            panic!("a cube of a dump was accepted")
        };
        assert!(said.contains("--cube"), "should say why: {said}");

        let sources = [Source::Eddn];
        assert!(
            Qualifiers { sphere: Some(50), ..reading(&sources) }
                .refused()
                .is_err(),
            "a sphere of the feed was accepted",
        );

        let sources = [Source::EdsmApi("Sol".to_owned())];
        assert!(Qualifiers { cube: Some(50), ..reading(&sources) }
            .refused()
            .is_ok());
    }

    /// A commander is whose journal is read, so a run must read one
    #[test]
    fn a_commander_needs_a_journal_to_be_read() {
        let sources = [Source::Eddn];
        let Err(said) =
            Qualifiers { user: Some("HRC-2"), ..reading(&sources) }.refused()
        else {
            panic!("a commander named over the feed was accepted")
        };
        assert!(said.contains("--user"), "should say why: {said}");

        let sources = [journal()];
        assert!(Qualifiers { user: Some("HRC-2"), ..reading(&sources) }
            .refused()
            .is_ok());
    }

    /// Nought said is a connection left alone, and nothing said is the
    /// window
    ///
    /// Three states in two, which is the one flag [`Qualifiers::options`]
    /// does not simply copy: a run that means "never replace this
    /// connection" and a run that said nothing must not come out the same.
    #[test]
    fn a_stall_of_nought_is_a_connection_left_alone() {
        let sources = [Source::Eddn];
        let unsaid = reading(&sources).options("test");
        assert_eq!(unsaid.stall, Some(eddn::STALL));

        let never =
            Qualifiers { stall: Some(0), ..reading(&sources) }.options("test");
        assert_eq!(never.stall, None, "0 is a connection left alone");

        let said =
            Qualifiers { stall: Some(45), ..reading(&sources) }.options("test");
        assert_eq!(said.stall, Some(Duration::from_secs(45)));
    }
}
