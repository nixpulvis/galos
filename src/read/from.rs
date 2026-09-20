//! Which publisher a run reads from.
//!
//! One flag, `--from`, repeated once per publisher. It is written where it is
//! read — `--from journal=PATH` — rather than as a subcommand with a
//! positional argument, because a run reads from several at once now and a
//! subcommand can only be given once: `--from eddn --from journal=PATH` is a
//! commander's own game and everybody else's landing in the same index.
//!
//! What a source names is the whole of how to reach it, so nothing here needs
//! a second flag to make sense of. The per-source options that remain on the
//! command line — `--user`, `--remote`, `--stall`, `--cube`, `--sphere` and
//! `--shard` — qualify a reading rather than name one, and each is refused
//! where the run reads no source it could qualify.
//!
//! The other thing a source says is whether it ends: see [`Source::follows`].
//! A run of sources that all end writes its directory once, when it ends; a
//! run that follows one writes on `--publish`'s beat as well.

use crate::Shard;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A publisher, and what it takes to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The live feed, over ZMQ. `--remote` says where, `--stall` says how
    /// long it may carry nothing before the connection is replaced.
    Eddn,
    /// A recorded feed, replayed off disk: the same messages in the same
    /// order, read from a directory `eddn record` is writing. See
    /// [`eddn::spool`].
    ///
    /// Where in it to start rides the source, the way every other
    /// source's argument does: `spool=DIR`, `spool=DIR,from=earliest`,
    /// `spool=DIR,since=2026-09-19T12:00:00Z`.
    Spool(PathBuf, eddn::spool::Start),
    /// A journal directory, or one file in one. `--user` overrides whose the
    /// files say it is; `--watch` keeps following it.
    Journal(PathBuf),
    /// An EDSM nightly dump already on disk.
    Edsm(PathBuf),
    /// EDSM's web API, asked about one system and its neighbourhood.
    /// `--cube` and `--sphere` say how much of the neighbourhood.
    EdsmApi(String),
    /// A saved EDDB dump. EDDB itself is gone, so there is no API to reach.
    Eddb(PathBuf),
    /// A Spansh galaxy dump: a system and every body in it, per line.
    Spansh(PathBuf),
    /// The rows, read back out into a directory.
    ///
    /// **A publisher like the others, and not like them.** An index is
    /// *rebuilt* from the database rather than maintained from it: what
    /// this reads is a query per part rather than a stream of events, it
    /// carries a cursor nothing else can offer — the clock the rows were
    /// written by — and it is the one source that cannot be read into the
    /// database, which is what it is. It is a `--from` anyway because
    /// naming it beside a feed is exactly the run that wants both: bring
    /// the directory level with the rows, then carry it forward from what
    /// arrives. See `galos::read::derive`.
    Database,
}

impl Source {
    /// Whether this source has no end of its own.
    ///
    /// `watching` is whether `--watch` was given, which is what the answer
    /// turns on for a journal directory: read once it is an import of the
    /// logs that are there, followed it is the game writing as it is
    /// flown. The feed never ends however it was asked for, and a dump, a
    /// saved dump and an API answer are all there when the run starts and
    /// read out by the time it finishes.
    /// Whether `--shard` divides this source between processes.
    ///
    /// **A file, and nothing else.** A share is every Nth record of what
    /// is already there, so it needs a thing with an Nth record: the two
    /// line-oriented dumps, the saved dump, and a journal directory,
    /// which shares by *file* because a file is what names the commander
    /// who flew it.
    ///
    /// The feed is a subscription — every subscriber is sent all of it —
    /// and the API answers about one system's neighbourhood. A spool is
    /// the feed written down, and its reader follows a cursor rather than
    /// counting records; the rows are queried per part. None of the four
    /// divides, and each reader would have taken the flag and quietly
    /// read the whole of its source, which is why this is asked here
    /// rather than left to whichever reader remembered to refuse.
    pub fn divides(&self) -> bool {
        match self {
            Source::Journal(_)
            | Source::Edsm(_)
            | Source::Eddb(_)
            | Source::Spansh(_) => true,
            Source::Eddn
            | Source::Spool(..)
            | Source::EdsmApi(_)
            | Source::Database => false,
        }
    }

    pub fn follows(&self, watching: bool) -> bool {
        match self {
            Source::Eddn | Source::Spool(..) => true,
            Source::Journal(_) => watching,
            // The rows have no end of their own either way: `--watch`
            // polls them and without it the pass reads what is there and
            // stops, which is the same rule a journal directory has.
            Source::Database => watching,
            Source::Edsm(_)
            | Source::EdsmApi(_)
            | Source::Eddb(_)
            | Source::Spansh(_) => false,
        }
    }
}

impl FromStr for Source {
    type Err = String;

    /// `eddn`, `journal=PATH`, `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH`,
    /// `spansh=PATH` or `database`.
    ///
    /// `journal:PATH` is taken too. A Windows path begins `C:\` and a reader
    /// who writes `--from journal:C:\…` should get the directory they meant
    /// rather than a complaint, so the split is on the *first* separator only
    /// and everything after it is the argument.
    fn from_str(said: &str) -> Result<Source, String> {
        let (kind, arg) = match said.split_once(['=', ':']) {
            Some((kind, arg)) => (kind, Some(arg)),
            None => (said, None),
        };
        let named = |arg: Option<&str>| match arg {
            Some(arg) if !arg.is_empty() => Ok(arg.to_string()),
            _ => Err(format!("`{}=` names nothing to read", kind.trim())),
        };
        match (kind.trim(), arg) {
            ("eddn", None) => Ok(Source::Eddn),
            ("eddn", Some(_)) => Err("`eddn` takes no path; the feed is \
                                      wherever --remote says"
                .to_string()),
            ("spool", arg) => spool(named(arg)?),
            ("journal", arg) => Ok(Source::Journal(path(named(arg)?))),
            ("edsm", arg) => Ok(Source::Edsm(path(named(arg)?))),
            ("edsm-api", arg) => Ok(Source::EdsmApi(named(arg)?)),
            ("eddb", arg) => Ok(Source::Eddb(path(named(arg)?))),
            ("spansh", arg) => Ok(Source::Spansh(path(named(arg)?))),
            ("database" | "db", None) => database(),
            ("database" | "db", Some(_)) => {
                Err("`database` takes no path; the connection is whatever \
                     DATABASE_URL names"
                    .to_string())
            }
            (other, _) => Err(format!(
                "unknown source `{other}`; expected `eddn`, `spool=DIR`, \
                 `journal=PATH`, `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH`, \
                 `spansh=PATH` or `database`"
            )),
        }
    }
}

/// The rows, where this build has a client to read them with.
///
/// Refused by name rather than missing from the grammar: a reader who
/// asks a copy built without the `db` feature to read the database has
/// asked for something coherent, and "unknown source" would send them
/// looking for a typo instead of at the build they are holding.
fn database() -> Result<Source, String> {
    #[cfg(feature = "db")]
    return Ok(Source::Database);
    #[cfg(not(feature = "db"))]
    Err("this build has no database in it; rebuild with the `db` feature, \
         or name a publisher"
        .to_string())
}

/// `DIR`, and where in it to start: `DIR,from=earliest|latest|cursor` or
/// `DIR,since=RFC3339`.
///
/// The default is this consumer's own cursor, which is a fresh follower
/// following and a restarted one taking up where it stopped. `earliest`
/// is the replay — a recorded hour read as a fixture — and `since` is
/// the one an operator reaches for after an outage they can name the
/// start of.
fn spool(said: String) -> Result<Source, String> {
    let mut parts = said.split(',');
    let dir = path(parts.next().unwrap_or_default().to_owned());
    let mut start = eddn::spool::Start::Cursor(CONSUMER.to_owned());
    for qualifier in parts {
        let (key, value) = qualifier.split_once('=').ok_or_else(|| {
            format!("`{qualifier}` is not `from=…` or `since=…`")
        })?;
        start = match (key.trim(), value.trim()) {
            ("from", "earliest") => eddn::spool::Start::Earliest,
            ("from", "latest") => eddn::spool::Start::Latest,
            ("from", "cursor") => {
                eddn::spool::Start::Cursor(CONSUMER.to_owned())
            }
            ("from", other) => {
                return Err(format!(
                    "`from={other}`: expected `earliest`, `latest` or \
                     `cursor`"
                ));
            }
            ("since", moment) => eddn::spool::Start::Since(
                moment
                    .parse::<chrono::DateTime<chrono::Utc>>()
                    .map_err(|err| format!("`since={moment}`: {err}"))?,
            ),
            (other, _) => {
                return Err(format!(
                    "`{other}=`: a spool takes `from=` and `since=`"
                ));
            }
        };
    }
    Ok(Source::Spool(dir, start))
}

/// What a run calls itself in a spool's `cursors/` directory, where it
/// did not say.
///
/// One name per consumer, so the `db` and `index` verbs keep their own
/// places in the same spool and neither can move the other's — which is
/// why each group passes its own and this is only the fallback for a
/// caller that is neither.
pub const CONSUMER: &str = "galos";

/// A path as written, with a leading `~` standing for the home directory.
///
/// The shell expands `~` only at the start of a word, so `--from
/// spansh=~/dumps/galaxy.json` arrives here with the tilde intact and no
/// amount of quoting on the reader's part would have helped. Every source
/// that names a file goes through this.
///
/// `~user` is not taken: this expands `~` and `~/…` and nothing else, so a
/// directory genuinely called `~something` still reads as itself.
fn path(said: String) -> PathBuf {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    match said.strip_prefix('~') {
        Some("") => home().unwrap_or_else(|| said.into()),
        Some(rest) if rest.starts_with('/') => match home() {
            Some(home) => home.join(rest.trim_start_matches('/')),
            None => said.into(),
        },
        _ => said.into(),
    }
}

/// What a file a publisher put out is filed under: the publisher and the
/// file's own name.
///
/// The name and not the path. `updated_by` is a column of a database and a
/// field of every body an index publishes, so what a run writes there is
/// read by people who have never seen the machine it ran on, and a
/// directory off that machine says nothing to any of them. A path with no
/// last component leaves the publisher standing alone.
pub fn published(publisher: &str, path: &Path) -> String {
    match path.file_name() {
        Some(name) => format!("{publisher} {}", name.to_string_lossy()),
        None => publisher.to_string(),
    }
}

/// Said back the way it was written, which is what a refusal quotes.
impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Source::Eddn => write!(f, "eddn"),
            Source::Spool(path, _) => write!(f, "spool={}", path.display()),
            Source::Journal(path) => write!(f, "journal={}", path.display()),
            Source::Edsm(path) => write!(f, "edsm={}", path.display()),
            Source::EdsmApi(name) => write!(f, "edsm-api={name}"),
            Source::Eddb(path) => write!(f, "eddb={}", path.display()),
            Source::Spansh(path) => write!(f, "spansh={}", path.display()),
            Source::Database => write!(f, "database"),
        }
    }
}

/// One share of a file, as `--shard` is written: `I/N`.
///
/// `I` is which share this process takes, counted from nought, and `N` is how
/// many are reading the file between them. Refused unless `I` is one of `N`:
/// a share after the last one and a share of nought shards are each a process
/// that reads no record of the file it was pointed at, and the whole point of
/// the flag is that `N` of them cover the file exactly once.
pub fn shard(said: &str) -> Result<Shard, String> {
    let Some((index, count)) = said.split_once('/') else {
        return Err(format!(
            "`{said}` is not a share; expected `I/N`, as `0/8`"
        ));
    };
    let number = |said: &str| {
        said.trim()
            .parse::<u64>()
            .map_err(|err| format!("`{}`: {err}", said.trim()))
    };
    let (index, count) = (number(index)?, number(count)?);
    if count == 0 {
        return Err("`/0` is no shards at all; N is one or more".to_string());
    }
    if index >= count {
        return Err(format!(
            "`{index}/{count}` names no share of {count}; I is nought to {}",
            count - 1
        ));
    }
    Ok(Shard { index, count })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `~` is the reader's home, since the shell will not have expanded it
    #[test]
    fn a_leading_tilde_is_the_home_directory() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());

        // The shell expands `~` only at the start of a word, so this is
        // exactly what `--from spansh=~/dumps/galaxy.json` hands over.
        assert_eq!(
            "spansh=~/dumps/galaxy.json".parse(),
            Ok(Source::Spansh(home.join("dumps/galaxy.json"))),
        );
        assert_eq!("journal=~".parse(), Ok(Source::Journal(home.clone())));

        // A tilde anywhere else is a character in a name.
        assert_eq!(
            "edsm=dumps/~odd/systems.json".parse(),
            Ok(Source::Edsm(PathBuf::from("dumps/~odd/systems.json"))),
        );
        assert_eq!(
            "eddb=~odd".parse(),
            Ok(Source::Eddb(PathBuf::from("~odd"))),
        );
    }

    /// The seven ways in, each naming what it takes
    #[test]
    fn a_source_is_named_with_what_it_takes() {
        assert_eq!("eddn".parse(), Ok(Source::Eddn));
        assert_eq!(
            "journal=/home/cmdr/journals".parse(),
            Ok(Source::Journal(PathBuf::from("/home/cmdr/journals"))),
        );
        assert_eq!(
            "edsm=systems.json".parse(),
            Ok(Source::Edsm(PathBuf::from("systems.json"))),
        );
        assert_eq!(
            "edsm-api=Sol".parse(),
            Ok(Source::EdsmApi("Sol".to_string())),
        );
        assert_eq!(
            "eddb=systems.csv".parse(),
            Ok(Source::Eddb(PathBuf::from("systems.csv"))),
        );
        assert_eq!(
            "spansh=galaxy.json".parse(),
            Ok(Source::Spansh(PathBuf::from("galaxy.json"))),
        );
        // The name in it is the fallback: a tool passes its own to
        // `Eddn::spooled`, which re-points the cursor before a message is
        // read. What the parse decides is only *that* it starts from a
        // cursor.
        assert_eq!(
            "spool=/var/lib/galos/spool".parse(),
            Ok(Source::Spool(
                PathBuf::from("/var/lib/galos/spool"),
                eddn::spool::Start::Cursor(CONSUMER.to_owned()),
            )),
        );
    }

    /// A spool says where in itself to start, and refuses what it cannot
    ///
    /// The default is the consumer's own cursor — a restart takes up
    /// where it stopped, and a first run follows rather than replaying
    /// two days at somebody who asked to be current. The other two are
    /// asked for on purpose, which is why a misspelling of either is a
    /// refusal rather than a quiet fall back to the default.
    #[test]
    fn a_spool_says_where_in_itself_to_start() {
        let since = "2026-09-19T12:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .expect("a moment");
        let dir = PathBuf::from("/spool");
        assert_eq!(
            "spool=/spool,from=earliest".parse(),
            Ok(Source::Spool(dir.clone(), eddn::spool::Start::Earliest)),
        );
        assert_eq!(
            "spool=/spool,from=latest".parse(),
            Ok(Source::Spool(dir.clone(), eddn::spool::Start::Latest)),
        );
        assert_eq!(
            "spool=/spool,since=2026-09-19T12:00:00Z".parse(),
            Ok(Source::Spool(dir, eddn::spool::Start::Since(since))),
        );

        assert!("spool=".parse::<Source>().is_err(), "a spool with no dir");
        assert!("spool=/spool,from=start".parse::<Source>().is_err());
        assert!("spool=/spool,since=lunchtime".parse::<Source>().is_err());
        assert!("spool=/spool,retain=48h".parse::<Source>().is_err());
    }

    /// A drive letter is part of the path, not a second separator
    ///
    /// `journal:C:\journals` splits once and the rest is the path. Split on
    /// every separator and the path comes back as `C`, which is a directory
    /// somebody would have found the hard way.
    #[test]
    fn a_windows_path_survives() {
        assert_eq!(
            r"journal:C:\journals".parse(),
            Ok(Source::Journal(PathBuf::from(r"C:\journals"))),
        );
    }

    /// A source that names nothing, or nothing this reads, says so
    #[test]
    fn a_source_with_nothing_to_read_is_refused() {
        assert!("journal".parse::<Source>().is_err());
        assert!("journal=".parse::<Source>().is_err());
        assert!("eddn=somewhere".parse::<Source>().is_err());
        assert!("postgres".parse::<Source>().is_err());
    }

    /// A file has an end and a feed does not, and a journal is told by
    /// `--watch`
    ///
    /// What the beat is decided by: a run of sources that all end has one
    /// publish, at the end, and everything written mid-run would be a
    /// directory rewritten whole for a dump that is still being read.
    #[test]
    fn only_a_source_with_no_end_is_followed() {
        let journal = Source::Journal(PathBuf::from("journals"));
        assert!(journal.follows(true), "--watch is what follows a journal");
        assert!(!journal.follows(false), "read once, it is an import");

        // The subscription never returns, asked for either way.
        assert!(Source::Eddn.follows(false));
        assert!(Source::Eddn.follows(true));

        // Dumps and an API answer are all there when the run starts.
        for source in [
            Source::Edsm(PathBuf::from("systems.json")),
            Source::EdsmApi("Sol".to_string()),
            Source::Eddb(PathBuf::from("systems.csv")),
            Source::Spansh(PathBuf::from("galaxy.json")),
        ] {
            assert!(!source.follows(true), "{} has an end", source);
        }
    }

    /// A share is `I/N`, and only where I is one of N
    ///
    /// `8/8` is the share after the last one and `1/0` is a share of nothing,
    /// and each would otherwise be a process reading no record of the file it
    /// was pointed at and saying it had read all of its share.
    #[test]
    fn a_share_names_one_of_several() {
        assert_eq!(shard("0/8"), Ok(Shard { index: 0, count: 8 }));
        assert_eq!(shard("7/8"), Ok(Shard { index: 7, count: 8 }));
        assert_eq!(shard("0/1"), Ok(Shard { index: 0, count: 1 }));
        assert!(shard("8/8").is_err());
        assert!(shard("1/0").is_err());
        assert!(shard("0/0").is_err());
        assert!(shard("4").is_err());
        assert!(shard("-1/8").is_err());
    }

    /// Every record falls to exactly one of the shards
    ///
    /// Which is the whole of what the flag promises: run N of them over one
    /// file and it is covered once between them, with each bar counting the
    /// share it is going to read.
    #[test]
    fn shards_cover_a_file_once_between_them() {
        let count = 5;
        let shards: Vec<Shard> =
            (0..count).map(|index| Shard { index, count }).collect();
        for at in 0..37 {
            assert_eq!(shards.iter().filter(|it| it.mine(at)).count(), 1);
        }
        assert_eq!(shards.iter().map(|it| it.share(37)).sum::<u64>(), 37);
        assert_eq!(shards[0].share(37), 8);
        assert_eq!(shards[4].share(37), 7);
    }
}
