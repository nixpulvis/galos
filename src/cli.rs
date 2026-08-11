//! Putting a [`View`] on standard output
//!
//! Everything here is about a terminal that is about to be exited from: what
//! it prints is read once, or piped into something that will read it once.
//! Which is why it ignores links entirely — there is nowhere to follow one
//! to — and why the same view drawn by [`crate::tui`] is a page you can move
//! around instead.
//!
//! The two formats are built as strings rather than printed as they go. A
//! view that is written a line at a time is a view that cannot be tested
//! without capturing a process's output, and `galos search … | head` is a
//! pipe closing halfway down a table.

use crate::query::Ask;
use crate::view::{Align, Section, Table, View};
use crate::Result;
use galos_db::Database;
use indicatif::{ProgressBar, ProgressStyle};
use prettytable::{format, Cell, Row};
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::time::Duration;

/// How much furniture to draw around the answer
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Ruled tables and headings, for reading
    Table,
    /// Tab separated columns and nothing else, for piping
    Plain,
}

/// Ask, and print what comes back
///
/// The spinner goes to standard error, so that a query slow enough to want
/// one can still have its answer piped somewhere.
pub async fn run(
    query: &impl Ask,
    db: &Database,
    format: Format,
) -> Result<()> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&[
                ">>><<<", ">>--<<", ">----<", "------", ">----<", ">>--<<",
                ">>><<<",
            ])
            .template("{spinner:.yellow} {msg}")
            .expect("a spinner template that parses"),
    );
    spinner.enable_steady_tick(Duration::from_millis(125));

    let asked = query.ask(db).await;
    spinner.finish_and_clear();

    print(&asked?, format);
    Ok(())
}

/// Write a view out
///
/// A closed pipe is not a failure. `galos search -s Col | head -5` closes
/// standard output partway through a table, and the answer to that is to stop
/// writing, not to print a panic over whatever `head` just showed the user.
pub fn print(view: &View, format: Format) {
    let out = io::stdout();
    let mut out = out.lock();
    match write!(out, "{}", rendered(view, format)).and_then(|_| out.flush()) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {}
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

/// A view as the text that would be printed
pub fn rendered(view: &View, format: Format) -> String {
    match format {
        Format::Table => ruled(view),
        Format::Plain => plain(view),
    }
}

/// For reading: a heading, then the sections, then what it comes to
fn ruled(view: &View) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", view.title);

    for section in &view.body {
        out.push('\n');
        match section {
            Section::Fields(fields) => {
                let width = fields.width();
                for field in fields.iter() {
                    let _ = writeln!(
                        out,
                        "  {:width$}  {}",
                        field.name,
                        field.value,
                        width = width
                    );
                }
            }
            Section::Table(table) => {
                if let Some(caption) = &table.caption {
                    let _ = writeln!(out, "{caption}");
                }
                if table.is_empty() {
                    let _ = writeln!(out, "  {}", table.empty);
                } else {
                    let _ = write!(out, "{}", printed(table));
                }
            }
            Section::Note(note) => {
                let _ = writeln!(out, "{note}");
            }
        }
    }

    if let Some(note) = &view.note {
        let _ = writeln!(out, "\n{note}");
    }

    out
}

/// For piping: the rows, a tab between the columns, and no headings
///
/// Headings and rules are for eyes. `galos search -s Col --format plain | cut
/// -f1` wants the names and nothing else, and every line that is not a row is
/// a line that has to be skipped past first.
fn plain(view: &View) -> String {
    let mut out = String::new();
    for section in &view.body {
        match section {
            Section::Fields(fields) => {
                for field in fields.iter() {
                    let _ = writeln!(out, "{}\t{}", field.name, field.value);
                }
            }
            Section::Table(table) => {
                for row in &table.rows {
                    let _ = writeln!(out, "{}", row.cells.join("\t"));
                }
            }
            // Notes are prose about the answer rather than part of it, and a
            // "no systems found" in the middle of a stream of rows is a row
            // that parses as a system called that.
            Section::Note(_) => {}
        }
    }
    out
}

/// One of our tables as one of prettytable's
fn printed(table: &Table) -> prettytable::Table {
    let mut printed = prettytable::Table::new();
    printed.set_format(*format::consts::FORMAT_NO_LINESEP_WITH_TITLE);
    printed.set_titles(Row::new(
        table
            .columns
            .iter()
            .map(|column| {
                Cell::new(&column.name).style_spec(spec(column.align))
            })
            .collect(),
    ));

    for row in &table.rows {
        printed.add_row(Row::new(
            row.cells
                .iter()
                .zip(table.columns.iter().map(|column| column.align))
                // A row with fewer cells than the table has columns is drawn
                // short rather than dropped; `zip` stopping at the shorter of
                // the two is what makes the "… and 12 more" row possible
                // without padding it out to the full width.
                .map(|(cell, align)| Cell::new(cell).style_spec(spec(align)))
                .collect(),
        ));
    }

    printed
}

/// prettytable says which way a cell leans in one letter
fn spec(align: Align) -> &'static str {
    match align {
        Align::Left => "l",
        Align::Right => "r",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Info;
    use crate::view::{Column, Fields, Row as ViewRow, Table as ViewTable};

    fn systems() -> View {
        let mut table = ViewTable::new([
            Column::text("System"),
            Column::number("Population"),
        ]);
        table.push(
            ViewRow::new(["Sol".into(), "22,780,919,531".into()])
                .linking(crate::query::Query::Info(Info::of("Sol"))),
        );
        table.push(ViewRow::new(["Meliae".into(), "9,832,821".into()]));
        View::new("Systems matching *")
            .with(Fields::new().and("asked", "*"))
            .with(table)
            .noting("2 systems")
    }

    /// Ruled output leads with the title and closes with the note
    #[test]
    fn a_printed_view_is_bracketed_by_what_it_is() {
        let out = rendered(&systems(), Format::Table);
        assert!(out.starts_with("Systems matching *\n"), "{}", out);
        assert!(out.trim_end().ends_with("2 systems"), "{}", out);
    }

    /// Plain output is rows and nothing else
    ///
    /// One line per row, tabs between the cells, and no title, rule, heading
    /// or note to skip past first.
    #[test]
    fn plain_output_is_only_the_answer() {
        let out = rendered(&systems(), Format::Plain);
        assert_eq!(out, "asked\t*\nSol\t22,780,919,531\nMeliae\t9,832,821\n");
    }

    /// Where a row leads is the UI's business, not the printer's
    #[test]
    fn links_are_not_printed() {
        for format in [Format::Table, Format::Plain] {
            assert!(!rendered(&systems(), format).contains("galos info"));
        }
    }

    /// An empty table says what is missing rather than ruling off nothing
    #[test]
    fn an_empty_table_says_what_is_missing() {
        let view = View::new("Sol").with(
            ViewTable::new([Column::text("Station")]).or_else("no stations"),
        );
        assert!(rendered(&view, Format::Table).contains("no stations"));
        // Not in the piped output: it is prose, and a line of it in a stream
        // of rows is a station called "no stations".
        assert_eq!(rendered(&view, Format::Plain), "");
    }
}
