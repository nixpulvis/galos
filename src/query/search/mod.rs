//! Looking for something by name, whatever kind of thing it is
//!
//! `galos search Meliae` does not ask what a Meliae is. It asks every kind of
//! thing that has a name whether anything of theirs is called that, and shows
//! what came back a kind at a time. Which is how someone holding a name and
//! no idea what it belongs to holds it — read off a screen in the game, or
//! half remembered — and that is the search worth having at the front.
//!
//! Narrowing is done by naming a kind: `galos search system Meliae`. What
//! that buys is the filters only that kind has, and all of what it found
//! rather than the first few.
//!
//! The flags up here are the ones that mean the same thing to every kind —
//! how many to answer with, and whether to answer with a count rather than a
//! list. They are [`global`](clap::Arg::global), so `search -l 5 system Sol`
//! and `search system Sol -l 5` are one line typed two ways. A filter only
//! one kind understands belongs to that kind's subcommand, where it can say
//! what it means: a radius is measured from a system, and a faction has
//! nowhere for one to be measured from.

use super::{quoted, Ask, Query};
use crate::view::{Section, Table, View};
use crate::{Error, Result};
use galos_db::Database;

pub mod body;
pub mod faction;
pub mod station;
pub mod system;

pub use self::body::Bodies;
pub use self::faction::Factions;
pub use self::station::Stations;
pub use self::system::Systems;

/// How many of each kind the search across all of them shows
///
/// Few, because the point of that page is which kinds have anything at all.
/// Whichever one turns out to be the answer is a keypress away with the whole
/// of its results, on a row that says which command asks for them.
const GLIMPSE: usize = 5;

/// Something to look for
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Search {
    /// The kind of thing to look for, where it is worth narrowing to one
    #[command(subcommand)]
    pub kind: Option<Kind>,

    /// What to look for, in anything that has a name
    ///
    /// Given without a flag, since it is what a search is. The rest of the
    /// line says how to read it.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// How many to answer with
    #[arg(
        short = 'l',
        long,
        global = true,
        default_value_t = 25,
        value_name = "N"
    )]
    pub limit: i64,

    /// Say how many there are rather than which they are
    ///
    /// Counted out of what `--limit` admits rather than out of the database,
    /// which has no cheaper way of being asked, so a count that reaches the
    /// limit says that it did.
    #[arg(short = 'c', long, global = true)]
    pub count: bool,
}

/// One kind of thing to look through
///
/// Each carries the filters that only make sense of it, and the name it is
/// looking for. What they have in common is on [`Search`] above them.
#[derive(clap::Subcommand, Clone, Debug, PartialEq)]
pub enum Kind {
    /// Star systems
    System(Systems),
    /// Minor factions
    Faction(Factions),
    /// Ports, outposts and carriers
    Station(Stations),
    /// Planets, moons and stars
    Body(Bodies),
}

/// What each kind of search has to be able to do
///
/// Answering with a [`Table`] rather than a [`View`] is what lets the search
/// across all kinds put four of them on one page while a kind's own
/// subcommand puts one of them on a page of its own. Either way the columns
/// were decided in one place, by the kind they are about.
pub(crate) trait Look {
    /// What one of this kind is called, capitalised, singular
    ///
    /// Said of several of them by [`many`], so that four kinds do not each
    /// carry two spellings of themselves and a body does not become a bodie.
    fn kind(&self) -> &'static str;

    /// What to head the page with
    fn title(&self) -> String;

    /// Everything of this kind matching, up to `limit`
    fn look(
        &self,
        db: &Database,
        limit: i64,
    ) -> impl std::future::Future<Output = Result<Table>> + Send;

    /// This kind and its filters, as they are written on a command line
    ///
    /// The `galos search` in front of them is [`Search`]'s to write, since
    /// the flags that follow are its as well.
    fn arguments(&self) -> String;
}

impl Ask for Search {
    async fn ask(&self, db: &Database) -> Result<View> {
        match &self.kind {
            Some(Kind::System(look)) => self.one(db, look).await,
            Some(Kind::Faction(look)) => self.one(db, look).await,
            Some(Kind::Station(look)) => self.one(db, look).await,
            Some(Kind::Body(look)) => self.one(db, look).await,
            None => self.everything(db).await,
        }
    }

    fn command(&self) -> String {
        let mut line = String::from("galos search");
        match &self.kind {
            Some(Kind::System(look)) => line += &look.arguments(),
            Some(Kind::Faction(look)) => line += &look.arguments(),
            Some(Kind::Station(look)) => line += &look.arguments(),
            Some(Kind::Body(look)) => line += &look.arguments(),
            None => {
                if let Some(name) = &self.name {
                    line += &format!(" {}", quoted(name));
                }
            }
        }
        if self.limit != 25 {
            line += &format!(" -l {}", self.limit);
        }
        if self.count {
            line += " -c";
        }
        line
    }
}

impl Search {
    /// Look for `name` in every kind of thing there is
    pub fn for_anything(name: &str) -> Self {
        Search { kind: None, name: Some(name.to_string()), ..Search::plain() }
    }

    /// Narrow to one kind, however that kind has been asked
    pub fn for_kind(kind: Kind) -> Self {
        Search { kind: Some(kind), ..Search::plain() }
    }

    /// The systems a faction is present in
    ///
    /// Where a faction row leads, and the reason a faction search answers
    /// with names rather than with the systems behind them: the systems are
    /// one keypress away and are their own page when you get there.
    pub fn systems_of(faction: &str) -> Self {
        Search::for_kind(Kind::System(Systems::held_by(faction)))
    }

    /// A search with the shared flags left where they are found
    fn plain() -> Self {
        Search { kind: None, name: None, limit: 25, count: false }
    }

    /// One kind, asked for all of what it has
    async fn one(&self, db: &Database, look: &impl Look) -> Result<View> {
        let found = look.look(db, self.limit).await?;
        let shown = found.rows.len();

        // Cut exactly at the limit, the database was not asked whether there
        // was a next one, so neither a count nor a list can promise there
        // was not.
        let note = if shown as i64 == self.limit {
            format!("{}, up to the --limit", tallied(look.kind(), shown))
        } else {
            tallied(look.kind(), shown)
        };

        Ok(if self.count {
            View::new(look.title()).with(Section::Note(note))
        } else {
            View::new(look.title()).with(found).noting(note)
        })
    }

    /// Every kind, asked for a glimpse of what it has
    ///
    /// A kind with nothing is left off the page rather than given a heading
    /// and an empty frame. Four "nothing found"s around one answer reads as a
    /// search that mostly failed, where the answer is the whole of what was
    /// asked for.
    async fn everything(&self, db: &Database) -> Result<View> {
        let Some(name) = &self.name else {
            return Err(Error::Nonsense(
                "search needs something to look for".into(),
            ));
        };

        let mut page = Page {
            view: View::new(format!("Anything matching {name}")),
            totals: vec![],
            listing: !self.count,
            limit: self.limit,
        };

        // Written out one after another rather than walked as a list. The
        // four are different types with different `look` futures, and the
        // only way to hold them in one collection is to box every one of
        // them for the sake of a loop that runs four times.
        page.add(db, Systems::named(name)).await?;
        page.add(db, Factions::named(name)).await?;
        page.add(db, Stations::named(name)).await?;
        page.add(db, Bodies::named(name)).await?;

        Ok(if page.totals.is_empty() {
            page.view
                .with(Section::Note(format!("nothing is called {name}")))
                .noting("nothing found")
        } else {
            let totals = page.totals.join(", ");
            page.view.noting(totals)
        })
    }
}

/// The page a search across every kind is building up
///
/// Held together so that adding a kind to it is one call rather than the same
/// eight lines four times over.
struct Page {
    view: View,
    totals: Vec<String>,
    /// Whether the rows go on the page, or only what they came to
    listing: bool,
    limit: i64,
}

impl Page {
    async fn add(
        &mut self,
        db: &Database,
        look: impl Look + Clone + Into<Kind>,
    ) -> Result<()> {
        let found = look.look(db, self.limit).await?;
        let total = found.rows.len();
        if total == 0 {
            return Ok(());
        }

        self.totals.push(tallied(look.kind(), total));
        if self.listing {
            let heading = if total > GLIMPSE {
                format!("{} ({GLIMPSE} of {total})", many(look.kind()))
            } else {
                many(look.kind())
            };
            let whole = Query::Search(Search::for_kind(look.into()));
            self.view
                .push(super::clipped(found, GLIMPSE, whole).captioned(heading));
        }
        Ok(())
    }
}

/// A kind's name, said of more than one of them
pub(crate) fn many(kind: &str) -> String {
    match kind.strip_suffix('y') {
        Some(stem) => format!("{stem}ies"),
        None => format!("{kind}s"),
    }
}

/// How many of a kind, said in words
fn tallied(kind: &str, count: usize) -> String {
    if count == 1 {
        format!("1 {}", kind.to_lowercase())
    } else {
        format!("{} {}", super::tally(count as u64), many(kind).to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A kind is spelled once and pluralised here
    #[test]
    fn a_body_is_never_a_bodie() {
        assert_eq!(many("System"), "Systems");
        assert_eq!(many("Faction"), "Factions");
        assert_eq!(many("Station"), "Stations");
        assert_eq!(many("Body"), "Bodies");
    }

    /// One of something is one of it, not one of them
    #[test]
    fn one_of_a_kind_is_singular() {
        assert_eq!(tallied("Body", 1), "1 body");
        assert_eq!(tallied("Body", 2), "2 bodies");
        assert_eq!(tallied("System", 1), "1 system");
        assert_eq!(tallied("System", 1_200), "1,200 systems");
    }
}
