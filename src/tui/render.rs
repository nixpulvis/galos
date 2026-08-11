//! Laying a [`View`] out as lines of a terminal
//!
//! The whole page becomes one list of styled lines and is drawn as a
//! paragraph, rather than each section becoming a widget of its own. A view
//! holds however many tables and however many field lists a command felt like
//! answering with, and a stack of independently scrolling widgets would mean
//! the page scrolls in pieces. One list scrolls as a page does.
//!
//! Which also means the cursor is a line number. Following a row and drawing
//! a row are then asking the same question of the same list, and there is
//! nowhere for the highlighted row and the followed row to come apart.

use crate::view::{Align, Section, Stop, Table, View};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// A page as it will be drawn
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    /// Which line the cursor sits on, where the page has one
    pub cursor: Option<usize>,
}

/// What the narrowest column is worth drawing at
///
/// Under this a column shows a letter and an ellipsis, which is not a column,
/// it is an apology. A page too narrow for all of its columns loses the
/// rightmost ones instead.
const NARROWEST: usize = 6;

/// Lay `view` out for a terminal `width` columns wide
///
/// `at` is the row the cursor is on, which is a [`Stop`] rather than a line
/// number: what the user moves over is rows that lead somewhere, and how many
/// lines of caption and heading sit between two of them is this function's
/// business rather than theirs.
pub fn render(view: &View, at: Option<Stop>, width: u16) -> Rendered {
    let width = width as usize;
    let mut lines = vec![];
    let mut cursor = None;

    for (index, section) in view.body.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        match section {
            Section::Fields(fields) => {
                let name = fields.width();
                for field in fields.iter() {
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("{:name$}", field.name),
                            Style::new().dim(),
                        ),
                        Span::raw("  "),
                        Span::raw(field.value.clone()),
                    ]));
                }
            }

            Section::Table(table) => {
                if let Some(caption) = &table.caption {
                    lines.push(Line::styled(
                        caption.clone(),
                        Style::new().add_modifier(Modifier::BOLD),
                    ));
                }
                if table.is_empty() {
                    lines.push(Line::styled(
                        format!("  {}", table.empty),
                        Style::new().dim(),
                    ));
                    continue;
                }

                let widths = widths(table, width);
                lines.push(Line::styled(
                    laid_out(
                        table.columns.iter().map(|c| c.name.as_str()),
                        table,
                        &widths,
                    ),
                    Style::new().dim().add_modifier(Modifier::UNDERLINED),
                ));

                for (row, drawn) in table.rows.iter().enumerate() {
                    let text = laid_out(
                        drawn.cells.iter().map(String::as_str),
                        table,
                        &widths,
                    );
                    let here = at == Some(Stop { section: index, row });
                    if here {
                        cursor = Some(lines.len());
                    }
                    lines.push(Line::styled(
                        text,
                        if here {
                            Style::new().add_modifier(Modifier::REVERSED)
                        } else {
                            Style::new()
                        },
                    ));
                }
            }

            Section::Note(note) => lines.push(Line::raw(note.clone())),
        }
    }

    Rendered { lines, cursor }
}

/// One row's cells, padded and spaced out under the headings
///
/// Takes the cells as an iterator so that a header row and a body row go
/// through the same code: they are the same shape and want the same widths,
/// and laying them out apart is how a heading ends up one space off the
/// column under it.
fn laid_out<'a>(
    cells: impl Iterator<Item = &'a str>,
    table: &Table,
    widths: &[usize],
) -> String {
    let mut out = String::new();
    for ((cell, width), column) in
        cells.zip(widths.iter()).zip(table.columns.iter())
    {
        if !out.is_empty() {
            out.push_str("  ");
        }
        out.push_str(&fit(cell, *width, column.align));
    }
    // Trailing space on the last column would be invisible, except under the
    // cursor, where the row is drawn in reverse and every space it holds is a
    // block of colour running off the end of the text.
    out.trim_end().to_string()
}

/// A cell at exactly `width` columns, padded or cut to get there
fn fit(cell: &str, width: usize, align: Align) -> String {
    let length = cell.chars().count();
    if length > width {
        let kept: String = cell.chars().take(width.saturating_sub(1)).collect();
        return format!("{kept}…");
    }
    match align {
        Align::Left => format!("{cell:width$}"),
        Align::Right => format!("{cell:>width$}"),
    }
}

/// How wide to draw each column, and how many of them there is room for
///
/// Wide enough for the widest thing in it, and then narrowed until the row
/// fits. Narrowed widest-first, so that a column of short words keeps them
/// while the column of system names gives up the space: taking a share from
/// every column would cut the short ones to nothing to spare the long one
/// something it can afford.
///
/// Only the columns of words are narrowed. A name cut to `Alpha…` is still
/// the name, read a little short, where a population cut to `22,78…` is not a
/// smaller population, it is a wrong one — the digits that were dropped are
/// the ones that said what the number was. So a column of numbers is drawn
/// whole or not drawn.
///
/// Which is what "not drawn" means here: short of room, the answer is fewer
/// columns rather than columns too narrow to be read. The rightmost go first,
/// the columns being written left to right in the order they are worth
/// reading. Never the last one standing — a table of no columns is not a
/// narrow table, it is a missing answer.
///
/// So the result can be shorter than the table has columns, and both the
/// header and the rows are laid out through it.
fn widths(table: &Table, width: usize) -> Vec<usize> {
    let mut widths: Vec<usize> = table
        .columns
        .iter()
        .enumerate()
        .map(|(i, column)| {
            table
                .rows
                .iter()
                .filter_map(|row| row.cells.get(i))
                .map(|cell| cell.chars().count())
                .chain([column.name.chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();

    let taken = |widths: &[usize]| {
        widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1)
    };

    while taken(&widths) > width {
        let widest = widths
            .iter()
            .enumerate()
            .filter(|(i, width)| {
                **width > NARROWEST && table.columns[*i].align == Align::Left
            })
            .max_by_key(|(_, width)| **width)
            .map(|(i, _)| i);
        match widest {
            Some(i) => widths[i] -= 1,
            None if widths.len() > 1 => {
                widths.pop();
            }
            None => break,
        }
    }

    widths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Column, Fields, Row, Table, View};

    fn systems() -> View {
        let mut table =
            Table::new([Column::text("System"), Column::number("Population")]);
        for name in ["Sol", "Meliae", "Alpha Centauri"] {
            table.push(Row::new([name.to_string(), "1,000".to_string()]));
        }
        View::new("Systems").with(Fields::new().and("asked", "*")).with(table)
    }

    /// A cell shorter than its column is padded to it
    #[test]
    fn a_short_cell_is_padded() {
        assert_eq!(fit("Sol", 6, Align::Left), "Sol   ");
        assert_eq!(fit("1,000", 8, Align::Right), "   1,000");
    }

    /// A cell longer than its column is cut, and says it was
    #[test]
    fn a_long_cell_is_marked_where_it_was_cut() {
        assert_eq!(fit("Alpha Centauri", 6, Align::Left), "Alpha…");
    }

    /// Given room, a column is as wide as the widest thing in it
    #[test]
    fn a_column_fits_what_is_in_it() {
        let View { body, .. } = systems();
        let Section::Table(table) = &body[1] else { panic!("a table") };
        assert_eq!(
            widths(table, 80),
            vec!["Alpha Centauri".len(), "Population".len()]
        );
    }

    /// Short of room, the widest column gives up the space
    ///
    /// Taking a share from each would cut the population column, which cannot
    /// spare anything, to buy room for the names, which can.
    #[test]
    fn the_widest_column_gives_way_first() {
        let View { body, .. } = systems();
        let Section::Table(table) = &body[1] else { panic!("a table") };
        let narrowed = widths(table, 20);
        assert!(narrowed.iter().sum::<usize>() + 2 <= 20);
        let names = "Alpha Centauri".len() - narrowed[0];
        let population = "Population".len() - narrowed[1];
        assert!(
            names > population,
            "names gave up {} and population {}",
            names,
            population
        );
    }

    /// A terminal too narrow for all of them shows fewer, not narrower
    ///
    /// Cut to a letter and an ellipsis a column says nothing. Past the floor
    /// the rightmost column goes instead.
    #[test]
    fn a_column_is_dropped_rather_than_cut_to_nothing() {
        let View { body, .. } = systems();
        let Section::Table(table) = &body[1] else { panic!("a table") };
        let narrow = widths(table, 12);
        assert_eq!(narrow.len(), 1);
        assert!(narrow.iter().all(|width| *width >= NARROWEST));
    }

    /// A column of numbers is drawn whole or not drawn
    ///
    /// `22,780,919,531` cut to `22,78…` is not a population read short, it is
    /// a different number, so the column goes rather than being cut. Names
    /// give up their space first and read short without lying.
    #[test]
    fn a_number_is_never_cut_short() {
        let View { body, .. } = systems();
        let Section::Table(table) = &body[1] else { panic!("a table") };
        let whole = "Population".len();
        for width in 8..40 {
            let narrow = widths(table, width);
            match narrow.get(1) {
                // Drawn, and drawn whole.
                Some(population) => assert_eq!(*population, whole, "{width}"),
                // Or not drawn, which is the other half of the bargain.
                None => assert_eq!(narrow.len(), 1, "{width}"),
            }
        }
        // And it is the names that give up the space to pay for it.
        assert!(widths(table, 24)[0] < "Alpha Centauri".len());
    }

    /// The last column standing is never dropped
    ///
    /// A table drawn with no columns at all is not a narrow table, it is a
    /// missing answer, and a terminal three columns wide is the user's
    /// problem to fix rather than ours to answer nothing about.
    #[test]
    fn one_column_always_survives() {
        let View { body, .. } = systems();
        let Section::Table(table) = &body[1] else { panic!("a table") };
        assert_eq!(widths(table, 1).len(), 1);
    }

    /// A dropped column is dropped from the header too
    ///
    /// The header and the rows are laid out through the same widths, so a
    /// column that is not drawn is not headed either. Otherwise the last
    /// heading sits over nothing.
    #[test]
    fn the_header_loses_what_the_rows_lose() {
        let view = systems();
        let drawn = render(&view, None, 12);
        let heading = drawn
            .lines
            .iter()
            .map(|line| line.to_string())
            .find(|line| line.starts_with("System"))
            .expect("a heading");
        assert!(!heading.contains("Popul"), "{}", heading);
    }

    /// The line the cursor is reported on is the row it is on
    #[test]
    fn the_cursor_lands_on_its_row() {
        let view = systems();
        let drawn = render(&view, Some(Stop { section: 1, row: 1 }), 80);
        let line = drawn.cursor.expect("a cursor");
        assert!(drawn.lines[line].to_string().starts_with("Meliae"));
    }

    /// With nowhere for the cursor to be, no line is highlighted
    #[test]
    fn a_page_without_stops_has_no_cursor() {
        assert!(render(&systems(), None, 80).cursor.is_none());
    }

    /// A row is drawn without the padding running off its last column
    ///
    /// Under the cursor the row is drawn in reverse, and trailing spaces are
    /// a block of colour reaching across the empty half of the screen.
    #[test]
    fn a_row_does_not_trail_off() {
        let view = systems();
        let drawn = render(&view, None, 80);
        for line in &drawn.lines {
            let text = line.to_string();
            assert_eq!(text.trim_end(), text, "{text:?}");
        }
    }

    /// An empty table says so rather than drawing a heading over nothing
    #[test]
    fn an_empty_table_says_what_is_missing() {
        let view = View::new("Sol")
            .with(Table::new([Column::text("Station")]).or_else("no stations"));
        let drawn = render(&view, None, 80);
        assert!(drawn
            .lines
            .iter()
            .any(|l| l.to_string().contains("no stations")));
    }
}
