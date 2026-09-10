//! Which sink a source was told to write to.
//!
//! One flag, `--to`, on every source that reads events or system rows. It is
//! written where it is read — `--to index=.galos_index` — rather than as a
//! bare choice plus a separate directory flag, because the two are one
//! decision: an index sink without a directory is not a thing, and a database
//! sink with one is a flag that means nothing.
//!
//! `db` is the default, which is what this program has always done and what
//! every existing invocation of it expects.

use std::path::PathBuf;
use std::str::FromStr;

/// Where an index is written when `--to index` names no directory.
pub const INDEX_DIR: &str = ".galos_index";

/// Where the resume point is kept when nothing names one.
///
/// Outside the served directory on purpose: a checkpoint is the builder's
/// private business and holds every system at full precision, and a client
/// reading the index has no use for it and should not be served it.
pub const CHECKPOINT: &str = ".galos_checkpoint";

/// The sink a source writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum To {
    /// Postgres, through `galos_db`. Needs `DATABASE_URL`.
    Db,
    /// An index directory, through `galos_index`. Needs no database at all.
    Index(PathBuf),
}

impl FromStr for To {
    type Err = String;

    /// `db`, `index`, or `index=DIR`.
    ///
    /// `index:DIR` is taken too. A Windows path begins `C:\` and a reader who
    /// writes `--to index:C:\…` should get the directory they meant rather
    /// than a complaint, so the split is on the *first* separator only and
    /// everything after it is the path.
    fn from_str(said: &str) -> Result<To, String> {
        let (kind, dir) = match said.split_once(['=', ':']) {
            Some((kind, dir)) => (kind, Some(dir)),
            None => (said, None),
        };
        match (kind.trim(), dir) {
            ("db", None) => Ok(To::Db),
            ("db", Some(_)) => {
                Err("`db` takes no directory; the connection is \
                     DATABASE_URL"
                    .to_string())
            }
            ("index", None) => Ok(To::Index(PathBuf::from(INDEX_DIR))),
            ("index", Some(dir)) if !dir.is_empty() => {
                Ok(To::Index(PathBuf::from(dir)))
            }
            ("index", Some(_)) => {
                Err("`index=` names no directory".to_string())
            }
            (other, _) => Err(format!(
                "unknown sink `{other}`; expected `db` or `index=DIR`"
            )),
        }
    }
}

impl std::fmt::Display for To {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            To::Db => write!(f, "the database"),
            To::Index(dir) => write!(f, "{}", dir.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three ways a sink is named, and the default directory
    #[test]
    fn a_sink_is_named_where_it_is_read() {
        assert_eq!("db".parse(), Ok(To::Db));
        assert_eq!(
            "index".parse(),
            Ok(To::Index(PathBuf::from(INDEX_DIR))),
            "a bare `index` should take the default directory",
        );
        assert_eq!(
            "index=.galos_journal_index".parse(),
            Ok(To::Index(PathBuf::from(".galos_journal_index"))),
        );
        assert_eq!(
            "index:/srv/galos".parse(),
            Ok(To::Index(PathBuf::from("/srv/galos"))),
        );
    }

    /// A drive letter is part of the path, not a second separator
    ///
    /// `index:C:\galos` splits once and the rest is the directory. Split on
    /// every separator and the path comes back as `C`, which is a directory
    /// somebody would have found the hard way.
    #[test]
    fn a_windows_path_survives() {
        assert_eq!(
            r"index:C:\galos".parse(),
            Ok(To::Index(PathBuf::from(r"C:\galos"))),
        );
    }

    /// What cannot be a sink says so rather than defaulting
    #[test]
    fn an_unknown_sink_is_refused() {
        assert!("postgres".parse::<To>().is_err());
        assert!("index=".parse::<To>().is_err());
        assert!("db=somewhere".parse::<To>().is_err());
    }
}
