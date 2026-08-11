//! What a query answered with, in a shape neither frontend owns
//!
//! A query returns one of these rather than printing. Printing is what the
//! CLI does with it and drawing is what the terminal UI does with it, and the
//! two agree on what a system's columns are because there is one place that
//! says so.
//!
//! The alternative is each frontend reading a `Vec<System>` and deciding for
//! itself which fields are worth a column and what they are called. That is
//! the same decision made twice, and the second one drifts: a column added to
//! the table is missing from the list, and the list's header says something
//! else besides.
//!
//! Nothing here knows about terminals, widths, or colour. A [`Column`] says
//! which way its cells lean and no more than that, because how wide to draw
//! it is a question about the thing drawing it.

use crate::query::{Ask, Query};

/// One query's whole answer
pub struct View {
    /// What was asked, said back
    ///
    /// The terminal UI draws this as the page's heading, and the CLI leaves
    /// it out: a shell already has the command that produced the output one
    /// line above it.
    pub title: String,
    /// The answer itself, in the order it is read
    pub body: Vec<Section>,
    /// The line under it, where there is something to total up
    pub note: Option<String>,
}

impl View {
    /// An answer with a heading and nothing in it yet
    pub fn new(title: impl Into<String>) -> Self {
        View { title: title.into(), body: vec![], note: None }
    }

    /// Add a section to the end of the answer
    pub fn with(mut self, section: impl Into<Section>) -> Self {
        self.body.push(section.into());
        self
    }

    /// Say what the answer adds up to, under it
    pub fn noting(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Every row that leads somewhere, in the order they are drawn
    ///
    /// The terminal UI moves a cursor over exactly these, which is why they
    /// are counted here rather than there: a row a frontend can put a cursor
    /// on and a row a query can be made from are the same row, and a cursor
    /// that stops on rows which lead nowhere is a cursor that mostly does
    /// nothing.
    pub fn stops(&self) -> Vec<Stop> {
        let mut stops = vec![];
        for (section, part) in self.body.iter().enumerate() {
            if let Section::Table(table) = part {
                for (row, drawn) in table.rows.iter().enumerate() {
                    if drawn.link.is_some() {
                        stops.push(Stop { section, row });
                    }
                }
            }
        }
        stops
    }

    /// Where a stop leads
    pub fn link(&self, stop: Stop) -> Option<&Link> {
        match self.body.get(stop.section)? {
            Section::Table(table) => table.rows.get(stop.row)?.link.as_ref(),
            _ => None,
        }
    }
}

/// One row of one section, as a place a cursor can be
///
/// A pair rather than a flat index into the rows, because drawing asks the
/// question the other way round — "is this row the one" — once per row, and
/// a flat index would have to be counted back into a section every time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stop {
    pub section: usize,
    pub row: usize,
}

/// One part of an answer
pub enum Section {
    /// Many of a thing, a row each
    Table(Table),
    /// One thing, described
    Fields(Fields),
    /// A line of prose, where the answer is that there is nothing to show
    Note(String),
}

impl From<Table> for Section {
    fn from(table: Table) -> Self {
        Section::Table(table)
    }
}

impl From<Fields> for Section {
    fn from(fields: Fields) -> Self {
        Section::Fields(fields)
    }
}

/// Rows under a header
pub struct Table {
    /// What the rows are, where the title has not already said
    pub caption: Option<String>,
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// What to say in place of the rows when there are none
    ///
    /// Held rather than left to the frontends, since a table with nothing in
    /// it is the common answer and "no systems found" reads better than an
    /// empty frame either of them could draw instead.
    pub empty: String,
}

impl Table {
    /// A table of the given columns, with nothing in it yet
    pub fn new(columns: impl IntoIterator<Item = Column>) -> Self {
        Table {
            caption: None,
            columns: columns.into_iter().collect(),
            rows: vec![],
            empty: "nothing found".into(),
        }
    }

    /// Say what the rows are
    pub fn captioned(mut self, caption: impl Into<String>) -> Self {
        self.caption = Some(caption.into());
        self
    }

    /// Say what to show when there are no rows
    pub fn or_else(mut self, empty: impl Into<String>) -> Self {
        self.empty = empty.into();
        self
    }

    /// Add a row to the end
    pub fn push(&mut self, row: Row) {
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// A column's header, and which edge its cells sit against
pub struct Column {
    pub name: String,
    pub align: Align,
}

impl Column {
    /// A column of words
    pub fn text(name: impl Into<String>) -> Self {
        Column { name: name.into(), align: Align::Left }
    }

    /// A column of numbers
    ///
    /// Right, so that the digits of one row line up under the digits of the
    /// next and a column of distances can be read down rather than across.
    pub fn number(name: impl Into<String>) -> Self {
        Column { name: name.into(), align: Align::Right }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

/// One thing, a cell per column, and where following it leads
pub struct Row {
    pub cells: Vec<String>,
    /// The query this row is a way of asking
    ///
    /// This is the whole of what makes the terminal UI navigable, and it is
    /// deliberately a [`Query`] rather than anything of its own. Every place
    /// the UI can reach by pressing enter is a place the CLI can reach by
    /// being typed, because they are the same value; a drill-down that could
    /// only be arrived at interactively cannot exist.
    pub link: Option<Link>,
}

impl Row {
    /// A row that leads nowhere
    pub fn new(cells: impl IntoIterator<Item = String>) -> Self {
        Row { cells: cells.into_iter().collect(), link: None }
    }

    /// A row that leads to `query`
    pub fn linking(mut self, query: Query) -> Self {
        self.link = Some(Link(query));
        self
    }
}

/// Somewhere a row leads
///
/// A wrapper rather than the query itself, so that what a frontend is holding
/// says what it is for. `row.link` reads as a place to go; `row.query` would
/// read as the query the row came from, which is the one thing it is not.
pub struct Link(pub Query);

impl Link {
    /// The query to ask on following it
    pub fn query(&self) -> &Query {
        &self.0
    }

    /// How to say the same thing at a shell
    ///
    /// Shown by the terminal UI beside the row it would follow, which is the
    /// cheapest way to teach the CLI: the argument list for wherever the user
    /// is about to press enter is already on the screen.
    pub fn command(&self) -> String {
        self.0.command()
    }
}

/// One thing's fields, in the order they are worth reading
pub struct Fields(pub Vec<Field>);

impl Fields {
    pub fn new() -> Self {
        Fields(vec![])
    }

    /// Add a field
    pub fn and(
        mut self,
        name: impl Into<String>,
        value: impl ToString,
    ) -> Self {
        self.0.push(Field { name: name.into(), value: value.to_string() });
        self
    }

    /// Add a field, where there is one to add
    ///
    /// Most of what is known about a system is known about some systems, and
    /// a row of `security: -` for every column the database happens to hold
    /// buries the three that were filled in.
    pub fn maybe(
        self,
        name: impl Into<String>,
        value: Option<impl ToString>,
    ) -> Self {
        match value {
            Some(value) => self.and(name, value),
            None => self,
        }
    }

    /// The width of the widest name, for lining the values up under each other
    pub fn width(&self) -> usize {
        self.0.iter().map(|field| field.name.len()).max().unwrap_or(0)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Field> {
        self.0.iter()
    }
}

impl Default for Fields {
    fn default() -> Self {
        Fields::new()
    }
}

pub struct Field {
    pub name: String,
    pub value: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Info;

    /// A table of three rows, the middle one leading nowhere
    fn mixed() -> View {
        let mut table = Table::new([Column::text("System")]);
        table.push(
            Row::new(["Sol".to_string()]).linking(Query::Info(Info::of("Sol"))),
        );
        table.push(Row::new(["… 12 more".to_string()]));
        table.push(
            Row::new(["Meliae".to_string()])
                .linking(Query::Info(Info::of("Meliae"))),
        );
        View::new("Systems").with(Fields::new().and("asked", "Sol")).with(table)
    }

    /// The cursor stops on rows that lead somewhere and nowhere else
    ///
    /// A row without a link is drawn like any other, and a cursor that sat on
    /// one would be a cursor that does nothing when enter is pressed.
    #[test]
    fn only_rows_that_lead_somewhere_are_stops() {
        let view = mixed();
        assert_eq!(
            view.stops(),
            vec![Stop { section: 1, row: 0 }, Stop { section: 1, row: 2 },]
        );
    }

    /// A stop leads where its row said it would
    #[test]
    fn a_stop_is_the_query_its_row_carried() {
        let view = mixed();
        let stops = view.stops();
        assert_eq!(view.link(stops[1]).unwrap().command(), "galos info Meliae");
    }

    /// Fields are described rather than stopped on
    #[test]
    fn fields_are_not_stops() {
        let view = View::new("Sol").with(Fields::new().and("address", 1));
        assert!(view.stops().is_empty());
        assert!(view.link(Stop { section: 0, row: 0 }).is_none());
    }

    /// The widest name is what the values are lined up past
    #[test]
    fn fields_line_up_under_the_longest_name() {
        let fields =
            Fields::new().and("address", 1).and("allegiance", "Federation");
        assert_eq!(fields.width(), "allegiance".len());
    }

    /// What is not on record is not a field
    #[test]
    fn an_absent_field_is_left_out() {
        let fields = Fields::new()
            .maybe("security", None::<String>)
            .maybe("government", Some("Democracy"));
        assert_eq!(fields.iter().count(), 1);
    }
}
