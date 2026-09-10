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

use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Where an index is written when `--to index` names no directory.
pub const INDEX_DIR: &str = ".galos_index";

/// What a resume point is named beside the directory it resumes.
///
/// One per index directory, and derived from it rather than fixed, because
/// the file carries the whole editable tree of *that* directory: the served
/// payload downcasts a magnitude and buckets a temperature, so the tree is
/// rebuilt from the checkpoint and from nothing else. Two directories
/// sharing one resume point is either a run that refuses to start or a
/// directory rebuilt from the other's systems, and
/// `--to index=.galos_journal_index` beside `--to index=.galos_index` is the
/// ordinary thing to want.
///
/// Beside the directory rather than inside it, likewise on purpose: a
/// checkpoint is the builder's private business and holds every system at
/// full precision, and a client reading the index has no use for it and
/// should not be served it.
pub const CHECKPOINT_SUFFIX: &str = ".checkpoint";

/// The sink a source writes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum To {
    /// Postgres, through `galos_db`. Needs `DATABASE_URL`.
    Db,
    /// An index directory, through `galos_index`. Needs no database at all.
    Index(PathBuf),
}

impl To {
    /// Where the resume point of an index in `dir` goes.
    ///
    /// Whatever `--checkpoint` named, and `<dir>.checkpoint` where it named
    /// nothing — see [`CHECKPOINT_SUFFIX`] for why the default is derived
    /// from the directory rather than being one path for the whole program.
    /// An associated function rather than a method because a database sink
    /// has no resume point to ask about: what a run of it leaves behind is
    /// rows, and every caller of this has an index directory in hand.
    pub fn checkpoint(dir: &Path, named: Option<&Path>) -> PathBuf {
        if let Some(named) = named {
            return named.to_owned();
        }
        // Appended to the directory's own name rather than through
        // `set_extension`, which replaces one that `--to index=galos.d`
        // carries and would have two such directories sharing a file again.
        let mut name = dir.file_name().unwrap_or_default().to_owned();
        name.push(CHECKPOINT_SUFFIX);
        dir.with_file_name(name)
    }
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

    /// Two index directories are two resume points
    ///
    /// The whole reason the default is derived. One path for the program
    /// meant `--to index=.galos_journal_index` and `--to index=.galos_index`
    /// sharing the file that carries the editable tree, which is a run that
    /// refuses to start or one directory rebuilt from the other's systems.
    #[test]
    fn each_index_directory_has_its_own_resume_point() {
        assert_eq!(
            To::checkpoint(Path::new(INDEX_DIR), None),
            PathBuf::from(".galos_index.checkpoint"),
        );
        assert_eq!(
            To::checkpoint(Path::new(".galos_journal_index"), None),
            PathBuf::from(".galos_journal_index.checkpoint"),
        );
        assert_eq!(
            To::checkpoint(Path::new("/srv/galos/index"), None),
            PathBuf::from("/srv/galos/index.checkpoint"),
            "the resume point goes beside the directory, not inside it",
        );
        assert_eq!(
            To::checkpoint(
                Path::new(INDEX_DIR),
                Some(Path::new("/var/lib/galos.resume")),
            ),
            PathBuf::from("/var/lib/galos.resume"),
            "a named resume point is the one that was named",
        );
    }
}
