//! Which publisher a run reads from.
//!
//! One flag, `--from`, repeated once per publisher. It is written where it is
//! read — `--from journal=PATH` — rather than as a subcommand with a
//! positional argument, because a run reads from several at once now and a
//! subcommand can only be given once: `--from eddn --from journal=PATH` is a
//! commander's own game and everybody else's landing in the same index.
//!
//! What a source names is the whole of how to reach it, so nothing here needs
//! a second flag to make sense of. The three per-source options that remain
//! on the command line — `--user`, `--remote`, `--stall`, `--cube` and
//! `--sphere` — qualify a reading rather than name one.

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
}

impl FromStr for Source {
    type Err = String;

    /// `eddn`, `journal=PATH`, `edsm=PATH`, `edsm-api=NAME` or `eddb=PATH`.
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
            (other, _) => Err(format!(
                "unknown source `{other}`; expected `eddn`, \
                 `journal=PATH`, `edsm=PATH`, `edsm-api=NAME` or `eddb=PATH`"
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five ways in, each naming what it takes
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
}
