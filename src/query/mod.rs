//! Everything the user can ask, said once
//!
//! A [`Query`] is what was asked, apart from who is asking and what they mean
//! to do with the answer. The CLI builds one by parsing `argv`, the terminal
//! UI builds one by parsing the line typed into its command bar or by taking
//! the [`Link`](crate::view::Link) off the row under the cursor, and all three
//! roads lead to [`Query::ask`].
//!
//! The parser is the same parser. `Query` derives clap's [`Subcommand`], so
//! `galos search -s Sol -r 50` at a shell and `search -s Sol -r 50` typed at
//! the UI go through one grammar with one help text; a flag added here shows
//! up in both places or in neither.
//!
//! The other direction matters as much. Every row the UI can put a cursor on
//! carries the query that following it would ask, and [`Query::command`]
//! writes that query back out as the command line that would have asked it.
//! Nothing is reachable by pressing enter that could not have been typed, so
//! the interactive tool teaches the batch one rather than growing away from
//! it.

use crate::view::View;
use crate::{Error, Result};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use elite_journal::system::Coordinate;
use galos_db::{systems::System, Database};

mod bodies;
mod factions;
mod info;
mod route;
mod search;
mod stations;

pub use self::bodies::Bodies;
pub use self::factions::Factions;
pub use self::info::Info;
pub use self::route::Route;
pub use self::search::Search;
pub use self::stations::Stations;

/// Something to ask the database
#[derive(Subcommand, Clone, Debug, PartialEq)]
pub enum Query {
    /// Find systems by name, by faction, or by what is near one
    Search(Search),
    /// Everything on record about one system
    Info(Info),
    /// The bodies of a system
    Bodies(Bodies),
    /// The stations of a system
    Stations(Stations),
    /// Find factions by name
    Factions(Factions),
    /// Plot a route between two systems
    Route(Route),
}

/// Asking, and what asking answers with
///
/// One method rather than one per frontend, and a [`View`] rather than
/// printed output, is the whole of the arrangement: a command decides what it
/// found and what its columns are called, and stops there. Where that goes is
/// somebody else's question.
pub trait Ask {
    /// Written out rather than left as `async fn` so that `+ Send` can be
    /// said. The terminal UI asks on a task of its own so that it can go on
    /// drawing while the database thinks about it, and a future that cannot
    /// cross threads cannot be put on one.
    fn ask(
        &self,
        db: &Database,
    ) -> impl std::future::Future<Output = Result<View>> + Send;

    /// How to ask the same thing at a shell, `galos` and all
    fn command(&self) -> String;
}

impl Ask for Query {
    async fn ask(&self, db: &Database) -> Result<View> {
        match self {
            Query::Search(q) => q.ask(db).await,
            Query::Info(q) => q.ask(db).await,
            Query::Bodies(q) => q.ask(db).await,
            Query::Stations(q) => q.ask(db).await,
            Query::Factions(q) => q.ask(db).await,
            Query::Route(q) => q.ask(db).await,
        }
    }

    fn command(&self) -> String {
        match self {
            Query::Search(q) => q.command(),
            Query::Info(q) => q.command(),
            Query::Bodies(q) => q.command(),
            Query::Stations(q) => q.command(),
            Query::Factions(q) => q.command(),
            Query::Route(q) => q.command(),
        }
    }
}

/// A query on its own, for parsing a line that has no program name in front
///
/// The terminal UI's command bar is handed `search -s Sol`, where a shell
/// would have handed us `galos search -s Sol`. Rather than push a fake
/// argument on the front and hope nothing ever prints it, the same derived
/// grammar is asked for with [`clap::Command::no_binary_name`] set.
#[derive(Parser)]
#[command(
    name = "galos",
    // A line typed at the UI has no program name in front of it.
    no_binary_name = true,
    // Neither `--help` nor `help` belongs here. Both answer by printing a
    // page and exiting, which is a thing a shell can be left holding and a
    // full screen terminal cannot; `?` is the UI's own help, and it is drawn
    // from these same subcommands.
    disable_help_flag = true,
    disable_help_subcommand = true
)]
struct Line {
    #[command(subcommand)]
    query: Query,
}

impl Query {
    /// Read a query out of a line someone typed
    ///
    /// Split as a shell would split it, so that a system whose name has a
    /// space in it can be quoted the way it would be quoted anywhere else.
    /// The error is clap's own, which is to say it is the same "unexpected
    /// argument" the CLI would have given, wording included.
    pub fn parse_line(line: &str) -> Result<Self> {
        let words = shlex::split(line)
            .ok_or_else(|| Error::Nonsense("unbalanced quotes".into()))?;
        if words.is_empty() {
            return Err(Error::Nonsense("nothing to ask".into()));
        }

        let matches = Line::command()
            // The message is going into a line of a page we are drawing, not
            // into a terminal clap is about to exit from, so its escape codes
            // would be printed rather than obeyed.
            .color(clap::ColorChoice::Never)
            .try_get_matches_from(words)
            .map_err(|e| Error::Nonsense(first_line(&e.to_string())))?;
        Line::from_arg_matches(&matches)
            .map(|line| line.query)
            .map_err(|e| Error::Nonsense(first_line(&e.to_string())))
    }

    /// The names a line may start with, and what each of them does
    ///
    /// Read out of the derived grammar rather than listed again anywhere, so
    /// that whatever offers the user a list of commands offers the list its
    /// parser accepts, with the descriptions `--help` gives.
    pub fn summaries() -> Vec<(String, String)> {
        Line::command()
            .get_subcommands()
            .map(|command| {
                (
                    command.get_name().to_string(),
                    command
                        .get_about()
                        .map(|about| about.to_string())
                        .unwrap_or_default(),
                )
            })
            .collect()
    }
}

/// clap's errors carry their usage and a suggestion under the message itself,
/// which is right for a terminal it is exiting from and too much for the one
/// line the UI has to say it in.
fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or("bad query")
        .trim_start_matches("error: ")
        .to_string()
}

/// The one system that name means
///
/// Exactly, if anything is called exactly that, and otherwise the one system
/// holding it if only one does. Long names are the rule out here — `Col 285
/// Sector KR-V b4-2` — and a tool that only accepts them whole is a tool
/// nobody types at twice.
///
/// Stops short of picking a favourite. Two systems holding the fragment is an
/// unanswered question, and the commands that need one system say so rather
/// than quietly describing whichever sorted first.
pub(crate) async fn locate(db: &Database, name: &str) -> Result<System> {
    if let Ok(system) = System::fetch_by_name(db, name).await {
        return Ok(system);
    }

    let mut holding = System::search_by_name(db, name, None, 2).await?;
    match holding.len() {
        1 => Ok(holding.remove(0)),
        0 => Err(Error::Unknown { thing: "system", name: name.to_string() }),
        _ => Err(Error::Nonsense(format!(
            "several systems are named like {name}; \
             say which with `galos search -s {}`",
            quoted(name)
        ))),
    }
}

/// A number of things, in groups of three digits
///
/// Populations run to eleven figures and are read off the screen rather than
/// counted, so they are grouped. A thin space would be prettier and would not
/// survive being piped into `cut`.
pub(crate) fn tally(n: u64) -> String {
    let digits = n.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Where something is, to the light year
///
/// Coordinates are held to a fraction the galaxy has no use for. Three
/// figures apiece fits a column and still names one system out of every
/// other.
pub(crate) fn place(at: Coordinate) -> String {
    format!("{}, {}, {}", whole(at.x), whole(at.y), whole(at.z))
}

/// A coordinate to the light year, without a minus in front of nothing
///
/// Alpha Centauri is nine hundredths of a light year below the plane, and
/// `{:.0}` writes that as `-0`, which reads as a direction rather than as the
/// rounding it is.
fn whole(ly: f64) -> f64 {
    let rounded = ly.round();
    if rounded == 0. {
        0.
    } else {
        rounded
    }
}

/// Something known, or a mark saying it is not
///
/// Cells are not left blank. A blank column reads as a column of nothing
/// worth saying, where most of what the database holds is simply not on
/// record for most systems, and the two are worth telling apart.
pub(crate) fn or_dash(value: Option<impl ToString>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
}

/// How a name is written into a command line
///
/// System names hold spaces far more often than not, and a link the UI offers
/// as `galos info Alpha Centauri` is a link that does not work when typed.
pub(crate) fn quoted(name: &str) -> String {
    if name.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        format!("\"{}\"", name.replace('"', "\\\""))
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every query that can be asked, for the round trips below
    ///
    /// Each is a shape the [`command`](Ask::command) writers have a case for:
    /// a flag left at its default, a flag moved off it, a name with a space
    /// in it, and an argument that is not a flag at all.
    fn every_shape() -> Vec<Query> {
        vec![
            Query::Search(Search {
                system: Some("Sol".into()),
                faction: None,
                radius: Some(50.),
                limit: 25,
                count: false,
            }),
            Query::Search(Search::by_faction("Aegis Core")),
            Query::Search(Search {
                system: Some("Col 285".into()),
                faction: Some("Aegis Core".into()),
                radius: None,
                limit: 100,
                count: true,
            }),
            Query::Info(Info::of("Alpha Centauri")),
            Query::Bodies(Bodies::of("Sol")),
            Query::Stations(Stations::of("Alpha Centauri")),
            Query::Factions(Factions { name: "Aegis".into(), limit: 25 }),
            Query::Factions(Factions { name: "New LHS".into(), limit: 5 }),
            Query::Route(Route {
                start: "Wolf 397".into(),
                end: "Meliae".into(),
                range: 32.,
            }),
        ]
    }

    /// What a row offers as a command line is a command line that asks for it
    ///
    /// This is the whole arrangement in one assertion. Rows carry a query,
    /// the UI writes that query out as the command that would have asked it,
    /// and a user who types what they were shown has to arrive where the
    /// cursor was going to take them. Nothing else checks that the writers of
    /// `command` and the flags they are writing about have stayed in step.
    #[test]
    fn a_command_asks_what_it_came_from() {
        for query in every_shape() {
            let line = query.command();
            let typed = line
                .strip_prefix("galos ")
                .expect("a command line that starts with the program");
            assert_eq!(
                Query::parse_line(typed).expect("a line that parses"),
                query,
                "{line}"
            );
        }
    }

    /// A name with a space in it survives being written out and read back
    #[test]
    fn a_name_of_two_words_is_one_argument() {
        let query = Query::Info(Info::of("Alpha Centauri"));
        assert_eq!(query.command(), r#"galos info "Alpha Centauri""#);
        assert_eq!(
            Query::parse_line(r#"info "Alpha Centauri""#).unwrap(),
            query
        );
    }

    /// The UI's command bar and the shell parse the same language
    #[test]
    fn a_typed_line_is_the_cli() {
        let Query::Search(search) =
            Query::parse_line("search -s Sol -r 50 -l 10").unwrap()
        else {
            panic!("a search")
        };
        assert_eq!(search.system.as_deref(), Some("Sol"));
        assert_eq!(search.radius, Some(50.));
        assert_eq!(search.limit, 10);
    }

    /// A line that means nothing says so in one line
    ///
    /// clap writes a usage message and a suggestion under its errors, which
    /// is right for a shell it is exiting from and three lines too many for
    /// the one the UI has to say it in.
    #[test]
    fn a_bad_line_is_a_short_complaint() {
        let err = Query::parse_line("serch -s Sol").unwrap_err();
        let said = err.to_string();
        assert_eq!(said.lines().count(), 1, "{}", said);
        assert!(!said.starts_with("error: "), "{}", said);
    }

    /// Nothing typed is not a query
    #[test]
    fn an_empty_line_asks_nothing() {
        assert!(Query::parse_line("").is_err());
        assert!(Query::parse_line("   ").is_err());
    }

    /// A quote left open is the user's, not a panic
    #[test]
    fn an_unclosed_quote_is_an_error() {
        assert!(Query::parse_line(r#"info "Alpha"#).is_err());
    }

    /// The commands offered are the commands accepted
    #[test]
    fn the_summaries_are_the_grammar() {
        let summaries = Query::summaries();
        assert!(!summaries.is_empty());
        for (name, about) in &summaries {
            assert!(!about.is_empty(), "{} has nothing said about it", name);
            // Not that the bare name parses — `route` wants two systems
            // after it and rightly refuses without them — but that the
            // parser knows the name at all. A command listed for the user to
            // type and then not recognised when they type it is the drift
            // this is here to catch.
            if let Err(err) = Query::parse_line(name) {
                let said = err.to_string();
                assert!(
                    !said.contains("unrecognized subcommand"),
                    "{} is offered and not accepted: {}",
                    name,
                    said
                );
            }
        }
    }

    /// Digits are grouped in threes from the right
    #[test]
    fn a_tally_is_grouped() {
        assert_eq!(tally(0), "0");
        assert_eq!(tally(999), "999");
        assert_eq!(tally(1_000), "1,000");
        assert_eq!(tally(22_780_919_531), "22,780,919,531");
    }

    /// Only a name that needs quoting gets quoted
    #[test]
    fn a_plain_name_is_left_alone() {
        assert_eq!(quoted("Sol"), "Sol");
        assert_eq!(quoted("Alpha Centauri"), r#""Alpha Centauri""#);
    }

    /// What is not on record is marked as not on record
    #[test]
    fn nothing_known_is_a_dash() {
        assert_eq!(or_dash(None::<String>), "-");
        assert_eq!(or_dash(Some("Federation")), "Federation");
    }
}
