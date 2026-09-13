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
//! `--shard` — qualify a reading rather than name one.

use galos::Shard;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

/// A publisher, and what it takes to reach it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The live feed, over ZMQ. `--remote` says where, `--stall` says how
    /// long it may carry nothing before the connection is replaced.
    Eddn,
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
}

impl FromStr for Source {
    type Err = String;

    /// `eddn`, `journal=PATH`, `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH` or
    /// `spansh=PATH`.
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
            ("journal", arg) => Ok(Source::Journal(named(arg)?.into())),
            ("edsm", arg) => Ok(Source::Edsm(named(arg)?.into())),
            ("edsm-api", arg) => Ok(Source::EdsmApi(named(arg)?)),
            ("eddb", arg) => Ok(Source::Eddb(named(arg)?.into())),
            ("spansh", arg) => Ok(Source::Spansh(named(arg)?.into())),
            (other, _) => Err(format!(
                "unknown source `{other}`; expected `eddn`, \
                 `journal=PATH`, `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH` \
                 or `spansh=PATH`"
            )),
        }
    }
}

/// Said back the way it was written, which is what a refusal quotes.
impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Source::Eddn => write!(f, "eddn"),
            Source::Journal(path) => write!(f, "journal={}", path.display()),
            Source::Edsm(path) => write!(f, "edsm={}", path.display()),
            Source::EdsmApi(name) => write!(f, "edsm-api={name}"),
            Source::Eddb(path) => write!(f, "eddb={}", path.display()),
            Source::Spansh(path) => write!(f, "spansh={}", path.display()),
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

    /// The six ways in, each naming what it takes
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
