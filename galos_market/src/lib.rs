//! What the galaxy trades, read three ways
//!
//! A prototype. Three panes, each answering the one beside it: every
//! commodity there is, everywhere the picked one trades, and everything the
//! picked market trades. Reads only; the listeners are what write here.
//!
//! Nothing below knows what a window is. [`Markets`] is a plain struct of
//! what is on screen, the panes take a [`Ui`](egui::Ui) and that struct, and
//! going to the database is an [`Ask`] handed back to whoever called
//! [`Markets::draw`]. The map draws its chrome with the same egui through
//! `bevy_egui`, so moving these panes there is a matter of making the state a
//! resource and the asks messages.
use chrono::{DateTime, Utc};
use eframe::egui;
use market::{Commodity, Quote, Summary};
use std::cmp::Ordering;
use trade::{Comparison, Endpoint, Filters, Trade};

pub mod board;
pub mod commodities;
pub mod db;
pub mod market;
pub mod quotes;
pub mod trade;
pub mod trades;

pub use db::{Answer, Ask, Queries};

/// What a number is drawn in where it is the good end of a trade
///
/// A green and a red that hold up against either theme's background, rather
/// than egui's own, which are picked to stand out on the dark one alone.
pub const GOOD: egui::Color32 = egui::Color32::from_rgb(0x3f, 0xa2, 0x4f);

/// What a number is drawn in where it is the bad end of one
pub const BAD: egui::Color32 = egui::Color32::from_rgb(0xc0, 0x4a, 0x4a);

/// How tall one row of either table stands
///
/// The same in both, so that a market read one way lines up with the same
/// market read the other, and so that neither table's rows are the taller.
pub(crate) const ROW: f32 = 18.;

/// Stop egui ringing every row of a table that has been scrolled
///
/// Egui checks between one pass and the next whether a rectangle kept its
/// place while everything drawn in it changed identity, and draws a red
/// border round every rectangle where that happened. A table that lays out
/// only the rows it is over hands one row's rectangle to the next row every
/// time it scrolls, which answers that description exactly and means none of
/// what the check is for. Scrolling a table of ten thousand markets therefore
/// lights up most of what is on screen.
///
/// Egui knows: the false positive is [#8092], the right aligned cells it
/// fires hardest on are [#8343], and [#8316] is the fix. Both borders and the
/// warnings behind them are turned off here until that lands. See
/// `ISSUE-egui-table-id-warnings.md`.
///
/// This is one debug check of several. Two widgets genuinely sharing an id
/// are caught by a different one that stays on, and still say so.
///
/// [#8092]: https://github.com/emilk/egui/issues/8092
/// [#8343]: https://github.com/emilk/egui/issues/8343
/// [#8316]: https://github.com/emilk/egui/pull/8316
pub fn quiet_scrolled_tables(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.debug.warn_if_rect_changes_id = false;
    });
}

/// Which column the quotes stand in the order of
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum By {
    System,
    Station,
    Buy,
    Stock,
    Sell,
    Demand,
    Updated,
}

/// The order the quotes stand in
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sort {
    pub by: By,
    pub descending: bool,
}

impl Default for Sort {
    /// The best price paid first
    ///
    /// The question a market table is opened with is usually where to take
    /// what you are already carrying, and that is this order's first row.
    fn default() -> Self {
        Sort { by: By::Sell, descending: true }
    }
}

/// Which of the two questions the middle pane is answering
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    /// Everywhere one commodity trades
    #[default]
    Quotes,
    /// What is worth carrying, and where to
    Trades,
}

/// What order the trades stand in
///
/// Three genuinely different answers. The best margin is often four tons of
/// something; the best run is what a full hold is worth; the nearest is what
/// you would actually fly to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Rank {
    #[default]
    Margin,
    Haul,
    Distance,
}

/// The market whose whole board is open
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub market_id: i64,
    pub system_name: String,
    pub station_name: String,
}

/// Everything on screen
#[derive(Default)]
pub struct Markets {
    /// Every commodity traded anywhere
    pub commodities: Vec<Summary>,
    /// What is typed above the commodity list
    pub search: String,

    /// The commodity picked out of that list
    pub picked: Option<String>,
    /// Everywhere it trades
    pub quotes: Vec<Quote>,
    /// Which of those are shown, in the order they are shown
    ///
    /// Sorting ten thousand rows is not something to do again for every
    /// frame that draws thirty of them, so the order is kept until something
    /// it was made from changes.
    shown: Vec<usize>,
    pub sort: Sort,
    /// Show only markets with any of it to sell
    pub only_stocked: bool,
    /// Show only markets wanting any of it
    pub only_wanted: bool,
    /// What is typed to narrow the quotes to a system or station
    pub place: String,

    /// The market opened out of the quotes
    pub opened: Option<Opened>,
    /// Everything it trades
    pub board: Vec<Commodity>,

    /// Which question the middle pane is answering
    pub tab: Tab,
    /// Where a run starts, once one has been pinned
    pub from: Option<Endpoint>,
    /// The system a search is anchored on, where no market has been pinned
    ///
    /// What is typed rather than what was found, so a half typed name is not
    /// a search. Pinning a market takes precedence: that is a stricter
    /// version of the same question.
    pub near: String,
    /// Where it ends
    pub to: Option<Endpoint>,
    /// What the last search turned up
    pub trades: Vec<Trade>,
    /// What both pinned markets trade, where two are pinned
    pub comparison: Vec<Comparison>,
    /// Light years between the pinned pair
    pub apart: Option<f64>,
    pub filters: Filters,
    pub rank: Rank,

    /// What the database last refused to answer
    pub trouble: Option<String>,
}

impl Markets {
    /// The first thing to ask, before anything can be picked
    pub fn opening() -> Ask {
        Ask::Commodities
    }

    /// Fold an answer in, or drop it where it answers a question since moved on
    ///
    /// An answer is not late, it is about something else. Two commodities
    /// picked in quick succession are two questions of different sizes, and
    /// the slower one arriving second would otherwise fill the table with
    /// rows for a commodity the user is no longer looking at.
    pub fn take(&mut self, answer: Answer) {
        match answer {
            Answer::Commodities(commodities) => {
                self.commodities = commodities;
            }
            Answer::Quotes(name, quotes) => {
                if self.picked.as_deref() == Some(&name) {
                    self.quotes = quotes;
                    self.reorder();
                }
            }
            Answer::Board(market_id, board) => {
                if self
                    .opened
                    .as_ref()
                    .is_some_and(|o| o.market_id == market_id)
                {
                    self.board = board;
                }
            }
            // Both are dropped where the pins have moved on since, the same
            // as a commodity picked twice: a search that took a second to
            // come back is about wherever it was asked from, not wherever
            // the user is looking now.
            Answer::Trades(from, found) => {
                if self.from.as_ref().map(|end| end.market_id) == from
                    && self.to.is_none()
                {
                    self.trades = found;
                    self.reorder_trades();
                }
            }
            Answer::Near(origin, found) => {
                if self.from.is_none() && self.near.trim() == origin {
                    self.trades = found;
                    self.reorder_trades();
                }
            }
            Answer::Compare(here, there, rows, apart) => {
                let pinned = |end: &Option<Endpoint>, id| {
                    end.as_ref().is_some_and(|end| end.market_id == id)
                };
                if pinned(&self.from, here) && pinned(&self.to, there) {
                    self.comparison = rows;
                    self.apart = apart;
                }
            }
            Answer::Trouble(ask, said) => {
                self.trouble = Some(format!("{:?}: {}", ask, said));
            }
        }
    }

    /// Pick a commodity, emptying what was shown of the last one
    pub fn pick(&mut self, name: &str) -> Ask {
        self.picked = Some(name.to_owned());
        self.quotes.clear();
        self.shown.clear();
        Ask::Quotes(name.to_owned())
    }

    /// Open a market's whole board
    pub fn open(&mut self, quote: &Quote) -> Ask {
        self.opened = Some(Opened {
            market_id: quote.commodity.market_id,
            system_name: quote.system_name.clone(),
            station_name: quote.station_name.clone(),
        });
        self.board.clear();
        Ask::Board(quote.commodity.market_id)
    }

    /// Shut the board pane
    pub fn close(&mut self) {
        self.opened = None;
        self.board.clear();
    }

    /// Put whichever trade question the pins have left standing
    ///
    /// Both ends pinned is a comparison and no search at all. One end is a
    /// search bounded by where that end is. Neither is the galaxy board.
    pub fn search(&mut self) -> Ask {
        match (&self.from, &self.to) {
            (Some(from), Some(to)) => {
                self.comparison.clear();
                self.apart = None;
                Ask::Compare(from.market_id, to.market_id)
            }
            (Some(from), None) => {
                self.trades.clear();
                Ask::Trades(Some(from.market_id), self.filters.clone())
            }
            // A place to stand rather than a station to stand on: every
            // market around it, both ends of the run inside the same reach.
            (None, _) if !self.near.trim().is_empty() => {
                self.trades.clear();
                Ask::Near(self.near.trim().to_owned(), self.filters.clone())
            }
            _ => {
                self.trades.clear();
                Ask::Trades(None, self.filters.clone())
            }
        }
    }

    /// Pin both ends of a trade, which turns a search into a comparison
    pub fn pin(&mut self, from: Endpoint, to: Endpoint) -> Ask {
        self.from = Some(from);
        self.to = Some(to);
        self.search()
    }

    /// Pin one end, leaving the other to be searched for
    pub fn pin_from(&mut self, end: Endpoint) -> Ask {
        self.from = Some(end);
        self.to = None;
        self.tab = Tab::Trades;
        self.search()
    }

    /// Pin the far end
    pub fn pin_to(&mut self, end: Endpoint) -> Ask {
        self.to = Some(end);
        self.tab = Tab::Trades;
        self.search()
    }

    /// Put the trades in the order asked for
    ///
    /// Done here rather than in the query, since all three orderings are over
    /// rows already in hand and re-asking the database to sort them would
    /// cost a second for nothing.
    pub fn reorder_trades(&mut self) {
        let hold = self.filters.hold;
        match self.rank {
            Rank::Margin => {
                self.trades.sort_by(|a, b| b.margin().cmp(&a.margin()))
            }
            Rank::Haul => {
                self.trades.sort_by(|a, b| b.haul(hold).cmp(&a.haul(hold)))
            }
            // Nearest first, and anything with no distance to give goes last
            // rather than pretending to be next door.
            Rank::Distance => {
                self.trades.sort_by(|a, b| match (a.distance, b.distance) {
                    (None, None) => Ordering::Equal,
                    (None, Some(_)) => Ordering::Greater,
                    (Some(_), None) => Ordering::Less,
                    (Some(a), Some(b)) => {
                        a.partial_cmp(&b).unwrap_or(Ordering::Equal)
                    }
                })
            }
        }
    }

    /// The quotes to draw, in the order to draw them
    pub fn shown(&self) -> &[usize] {
        &self.shown
    }

    /// What the galaxy makes of the picked commodity
    pub fn summary(&self) -> Option<&Summary> {
        let picked = self.picked.as_deref()?;
        self.commodities.iter().find(|s| s.name == picked)
    }

    /// Draw the whole of it, and hand back what it wants asked
    ///
    /// The asks come back rather than going out from here so that the panes
    /// stay ignorant of how a question is put. Under `bevy` the same call is
    /// one system, and these become messages.
    ///
    /// Laid out inside whatever [`Ui`](egui::Ui) it is given rather than
    /// against the whole screen, so the three panes hold together wherever
    /// they are put: a window of their own here, and whatever the map has
    /// room for later.
    pub fn draw(&mut self, ui: &mut egui::Ui) -> Vec<Ask> {
        let mut asks = Vec::new();

        if let Some(trouble) = self.trouble.clone() {
            egui::Panel::top("trouble").show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(trouble).color(BAD));
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.small_button("x").clicked() {
                                self.trouble = None;
                            }
                        },
                    );
                });
            });
        }

        egui::Panel::left("commodities")
            .resizable(true)
            .default_size(220.)
            .show_inside(ui, |ui| {
                commodities::commodities(ui, self, &mut asks)
            });

        // Drawn before the middle pane, which is what leaves the middle one
        // the room the two beside it are not using.
        if self.opened.is_some() {
            egui::Panel::right("board")
                .resizable(true)
                .default_size(400.)
                .show_inside(ui, |ui| board::board(ui, self, &mut asks));
        }

        egui::CentralPanel::default().show_inside(ui, |ui| {
            // Which question the middle pane is answering. Two tabs rather
            // than a fourth pane, since both want the whole width for a
            // table and only one of them is ever being read.
            ui.horizontal(|ui| {
                for (tab, label) in
                    [(Tab::Quotes, "Quotes"), (Tab::Trades, "Trades")]
                {
                    if ui.selectable_label(self.tab == tab, label).clicked() {
                        self.tab = tab;
                    }
                }
            });
            ui.separator();

            match self.tab {
                Tab::Quotes => quotes::quotes(ui, self, &mut asks),
                Tab::Trades => trades::trades(ui, self, &mut asks),
            }
        });

        asks
    }

    /// Work out which quotes are shown and in what order
    ///
    /// Called by whatever changed one of the answers to that, rather than
    /// once a frame.
    pub fn reorder(&mut self) {
        let place = self.place.trim().to_lowercase();
        let mut shown: Vec<usize> = (0..self.quotes.len())
            .filter(|&i| {
                let quote = &self.quotes[i];
                if self.only_stocked && !quote.commodity.is_stocked() {
                    return false;
                }
                if self.only_wanted && !quote.commodity.is_wanted() {
                    return false;
                }
                place.is_empty()
                    || quote.system_name.to_lowercase().contains(&place)
                    || quote.station_name.to_lowercase().contains(&place)
            })
            .collect();

        let sort = self.sort;
        shown.sort_by(|&a, &b| {
            let (a, b) = (&self.quotes[a], &self.quotes[b]);
            let ordering = match sort.by {
                By::System => text(&a.system_name, &b.system_name, sort),
                By::Station => text(&a.station_name, &b.station_name, sort),
                By::Buy => number(price(a, By::Buy), price(b, By::Buy), sort),
                By::Stock => {
                    number(price(a, By::Stock), price(b, By::Stock), sort)
                }
                By::Sell => {
                    number(price(a, By::Sell), price(b, By::Sell), sort)
                }
                By::Demand => {
                    number(price(a, By::Demand), price(b, By::Demand), sort)
                }
                By::Updated => {
                    let (a, b) = (a.commodity.listed_at, b.commodity.listed_at);
                    if sort.descending { b.cmp(&a) } else { a.cmp(&b) }
                }
            };
            // Two markets quoting the same number are still two markets, and
            // which comes first should not depend on the order the database
            // handed them over in.
            ordering.then_with(|| {
                text(
                    &a.station_name,
                    &b.station_name,
                    Sort { by: By::Station, descending: false },
                )
            })
        });

        self.shown = shown;
    }
}

/// What a quote says in one of its numeric columns, where it says anything
///
/// A market that has run out quotes a buy price for nothing it holds, and a
/// market with no room left quotes a sell price it will not pay. Those are
/// not prices of zero, they are no price, and sorting cannot put them among
/// the real ones at either end.
fn price(quote: &Quote, by: By) -> Option<i32> {
    let commodity = &quote.commodity;
    match by {
        By::Buy => commodity.is_stocked().then_some(commodity.buy_price),
        By::Stock => commodity.is_stocked().then_some(commodity.stock),
        By::Sell => commodity.is_wanted().then_some(commodity.sell_price),
        By::Demand => commodity.is_wanted().then_some(commodity.demand),
        _ => None,
    }
}

/// Order two numbers, with nothing quoted coming last either way round
fn number(a: Option<i32>, b: Option<i32>, sort: Sort) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => {
            if sort.descending {
                b.cmp(&a)
            } else {
                a.cmp(&b)
            }
        }
    }
}

/// Order two names by what they say rather than how they are capitalised
///
/// System names are held upper case and station names as they were given, so
/// a case sensitive sort would file every station starting with a lower case
/// letter after all the rest.
fn text(a: &str, b: &str, sort: Sort) -> Ordering {
    let ordering = a
        .chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase));
    if sort.descending { ordering.reverse() } else { ordering }
}

/// A number with the thousands marked off
pub fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut said = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        said.push('-');
    }
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            said.push(',');
        }
        said.push(digit);
    }
    said
}

/// How long ago something was read, as roughly as it is worth saying
///
/// Market prices go stale in hours, so an age is what a timestamp is worth
/// here. Anything older than a day is only worth knowing the day count of.
pub fn since(then: DateTime<Utc>) -> String {
    let elapsed = Utc::now().signed_duration_since(then);
    let minutes = elapsed.num_minutes();
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{}m", minutes)
    } else if elapsed.num_hours() < 48 {
        format!("{}h", elapsed.num_hours())
    } else {
        format!("{}d", elapsed.num_days())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::trade::Side;

    /// Draw `contents` into a bare context and tessellate what it made
    ///
    /// Laying a widget out is not the half of it that goes wrong. Egui defers
    /// a color the caller did not give as a placeholder for the painter to
    /// answer, and one answered by another placeholder is caught nowhere
    /// until epaint meets it and panics. So the shapes are turned into
    /// triangles here, which is the step that looks.
    pub(crate) fn painted(mut contents: impl FnMut(&mut egui::Ui)) {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(window(), |ui| contents(ui));
        ctx.tessellate(output.shapes, output.pixels_per_point);
    }

    /// A window's worth of room to draw in
    ///
    /// A bare [`egui::RawInput`] names no screen at all, and a table given no
    /// height lays out every row it has rather than the handful it is over.
    /// Which rows those are is most of what a table does, so the room has to
    /// run out somewhere.
    fn window() -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0., 0.),
                egui::vec2(1200., 800.),
            )),
            ..Default::default()
        }
    }

    /// Every piece of text `contents` painted, in the order it was painted
    ///
    /// What a widget draws is the whole of what the user is told, and a table
    /// row that lays its own cells out has no label to be asked what it says.
    /// So this reads it back off the shapes.
    pub(crate) fn words(
        mut contents: impl FnMut(&mut egui::Ui),
    ) -> Vec<String> {
        let ctx = egui::Context::default();
        text_painted(&ctx.run_ui(window(), |ui| contents(ui)))
    }

    /// Read the text back off what a pass painted
    fn text_painted(output: &egui::FullOutput) -> Vec<String> {
        fn text_of(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().into()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_of(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut said = Vec::new();
        for shape in &output.shapes {
            text_of(&shape.shape, &mut said);
        }
        said
    }

    /// A trade, for the pane to draw and the orderings to sort
    fn trade(
        name: &str,
        buy: i32,
        stock: i32,
        sell: i32,
        demand: i32,
        distance: Option<f64>,
    ) -> Trade {
        Trade {
            name: name.into(),
            source: Endpoint {
                market_id: 1,
                system_name: "SOL".into(),
                station_name: "Abraham Lincoln".into(),
            },
            destination: Endpoint {
                market_id: 2,
                system_name: "LAVE".into(),
                station_name: "Lave Station".into(),
            },
            buy_price: buy,
            stock,
            sell_price: sell,
            demand,
            distance,
            read_at: Utc::now(),
        }
    }

    #[test]
    fn a_run_is_worth_the_least_of_stock_demand_and_hold() {
        // A spectacular margin on four tons is four tons of profit. This is
        // what stops the board being a list of them.
        let scarce = trade(
            "thargoidtissuesampletype9a",
            24_927,
            4,
            49_852_300,
            900,
            None,
        );
        assert_eq!(scarce.margin(), 49_827_373);
        assert_eq!(scarce.tons(720), 4);
        assert_eq!(scarce.haul(720), 199_309_492);

        // Demand can be the binding end just as well as stock.
        let wanted = trade("gold", 4_459, 5_000, 67_781, 300, None);
        assert_eq!(wanted.tons(720), 300);

        // And usually it is the hold.
        let plenty = trade("palladium", 4_784, 5_000, 71_208, 5_000, None);
        assert_eq!(plenty.tons(720), 720);
        assert_eq!(plenty.haul(720), 47_825_280);
    }

    #[test]
    fn ordering_by_run_is_not_ordering_by_margin() {
        // Two tons at a million a ton against a full hold at five thousand.
        // The first wins on margin by two hundred times and still comes to
        // less than two thirds of what the second earns.
        let mut markets = Markets {
            trades: vec![
                trade("scarce", 1, 2, 1_000_000, 2, Some(20_000.)),
                trade("bulk", 100, 5_000, 5_100, 5_000, Some(80.)),
            ],
            ..Default::default()
        };
        assert_eq!(markets.trades[0].haul(720), 1_999_998);
        assert_eq!(markets.trades[1].haul(720), 3_600_000);

        markets.rank = Rank::Margin;
        markets.reorder_trades();
        assert_eq!(markets.trades[0].name, "scarce");

        // Four tons of a fortune is worth less than a full hold of a living.
        markets.rank = Rank::Haul;
        markets.reorder_trades();
        assert_eq!(markets.trades[0].name, "bulk");

        markets.rank = Rank::Distance;
        markets.reorder_trades();
        assert_eq!(markets.trades[0].name, "bulk");
    }

    #[test]
    fn a_trade_with_nowhere_to_measure_sorts_last() {
        let mut markets = Markets {
            trades: vec![
                trade("waiting", 1, 100, 500, 100, None),
                trade("far", 1, 100, 400, 100, Some(9_000.)),
                trade("near", 1, 100, 300, 100, Some(12.)),
            ],
            rank: Rank::Distance,
            ..Default::default()
        };
        markets.reorder_trades();

        let order: Vec<_> =
            markets.trades.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(order, ["near", "far", "waiting"]);
    }

    #[test]
    fn a_run_is_only_a_run_where_one_end_sells_and_the_other_buys() {
        let stocked = Side {
            buy_price: 4_000,
            stock: 500,
            sell_price: 3_900,
            demand: 0,
            listed_at: Utc::now(),
        };
        let wanting = Side {
            buy_price: 0,
            stock: 0,
            sell_price: 9_000,
            demand: 800,
            listed_at: Utc::now(),
        };

        let out = Comparison {
            name: "gold".into(),
            here: stocked.clone(),
            there: wanting.clone(),
        };
        assert_eq!(out.out(), Some(5_000));
        // Nothing to buy at the far end, so there is no run back.
        assert_eq!(out.back(), None);

        // Neither end stocks it: not a trade in either direction, however
        // much both of them want it.
        let neither = Comparison {
            name: "gold".into(),
            here: wanting.clone(),
            there: wanting,
        };
        assert_eq!(neither.out(), None);
        assert_eq!(neither.back(), None);
    }

    #[test]
    fn what_the_pins_leave_standing_is_what_gets_asked() {
        let mut markets = Markets::default();
        let sol = Endpoint {
            market_id: 1,
            system_name: "SOL".into(),
            station_name: "Abraham Lincoln".into(),
        };
        let lave = Endpoint {
            market_id: 2,
            system_name: "LAVE".into(),
            station_name: "Lave Station".into(),
        };

        // Nothing pinned is the galaxy board.
        assert!(matches!(markets.search(), Ask::Trades(None, _)));

        // One end pinned bounds the search by where that end is.
        assert!(matches!(
            markets.pin_from(sol.clone()),
            Ask::Trades(Some(1), _)
        ));

        // Both ends is no search at all.
        assert_eq!(markets.pin_to(lave), Ask::Compare(1, 2));
    }

    #[test]
    fn a_place_to_stand_is_asked_for_where_no_market_is_pinned() {
        let mut markets = Markets::default();

        // Nothing typed and nothing pinned is still the galaxy board.
        assert!(matches!(markets.search(), Ask::Trades(None, _)));

        markets.near = "  sol ".into();
        match markets.search() {
            // Trimmed, since what was typed is not what gets asked.
            Ask::Near(origin, _) => assert_eq!(origin, "sol"),
            other => panic!("{:?}", other),
        }

        // A pinned market is the same question asked more precisely, so it
        // wins over somewhere merely typed.
        let end = Endpoint {
            market_id: 4,
            system_name: "SOL".into(),
            station_name: "Abraham Lincoln".into(),
        };
        assert!(matches!(markets.pin_from(end), Ask::Trades(Some(4), _)));
    }

    #[test]
    fn a_place_answered_after_the_box_moved_on_is_dropped() {
        let mut markets = Markets::default();
        let found = vec![trade("gold", 1, 100, 500, 100, Some(12.))];

        markets.near = "LAVE".into();
        markets.take(Answer::Near("SOL".into(), found.clone()));
        assert!(markets.trades.is_empty());

        markets.take(Answer::Near("LAVE".into(), found));
        assert_eq!(markets.trades.len(), 1);
    }

    #[test]
    fn a_search_answered_after_the_pins_moved_is_dropped() {
        let mut markets = Markets::default();
        let found = vec![trade("gold", 1, 100, 500, 100, Some(12.))];

        let elsewhere = Endpoint {
            market_id: 7,
            system_name: "LAVE".into(),
            station_name: "Lave Station".into(),
        };
        markets.pin_from(elsewhere);

        // Answered for the galaxy, but a market has since been pinned.
        markets.take(Answer::Trades(None, found.clone()));
        assert!(markets.trades.is_empty());

        markets.take(Answer::Trades(Some(7), found));
        assert_eq!(markets.trades.len(), 1);
    }

    #[test]
    fn the_trades_pane_paints_what_it_found() {
        let mut markets = Markets {
            tab: Tab::Trades,
            trades: vec![
                trade("palladium", 4_784, 5_000, 71_208, 5_000, Some(84.)),
                trade("waiting", 1, 100, 500, 100, None),
            ],
            ..Default::default()
        };

        let said = words(|ui| {
            markets.draw(ui);
        });
        assert!(said.iter().any(|word| word == "palladium"), "{:?}", said);
        // A full hold of it, rather than the per-ton margin.
        assert!(said.iter().any(|word| word == "47,825,280"), "{:?}", said);
        // Nothing to measure, and it says so rather than claiming zero.
        assert!(said.iter().any(|word| word == "?"), "{:?}", said);
    }

    #[test]
    fn the_trades_pane_paints_a_comparison_when_both_ends_are_pinned() {
        let mut markets = Markets {
            tab: Tab::Trades,
            apart: Some(84.),
            comparison: vec![Comparison {
                name: "gold".into(),
                here: Side {
                    buy_price: 4_000,
                    stock: 500,
                    sell_price: 0,
                    demand: 0,
                    listed_at: Utc::now(),
                },
                there: Side {
                    buy_price: 0,
                    stock: 0,
                    sell_price: 9_000,
                    demand: 800,
                    listed_at: Utc::now(),
                },
            }],
            ..Default::default()
        };
        markets.from = Some(Endpoint {
            market_id: 1,
            system_name: "SOL".into(),
            station_name: "Abraham Lincoln".into(),
        });
        markets.to = Some(Endpoint {
            market_id: 2,
            system_name: "LAVE".into(),
            station_name: "Lave Station".into(),
        });

        let said = words(|ui| {
            markets.draw(ui);
        });
        assert!(said.iter().any(|word| word == "84 ly apart"), "{:?}", said);
        // 5,000 a ton over 500 tons, which is all either end can manage.
        assert!(said.iter().any(|word| word == "2,500,000"), "{:?}", said);
    }

    /// A market trading one commodity, for the panes to draw
    pub(crate) fn quote(
        market_id: i64,
        system: &str,
        station: &str,
        buy: i32,
        stock: i32,
        sell: i32,
        demand: i32,
    ) -> Quote {
        Quote {
            system_name: system.into(),
            station_name: station.into(),
            commodity: commodity(market_id, "gold", buy, stock, sell, demand),
        }
    }

    pub(crate) fn commodity(
        market_id: i64,
        name: &str,
        buy: i32,
        stock: i32,
        sell: i32,
        demand: i32,
    ) -> Commodity {
        Commodity {
            market_id,
            name: name.into(),
            mean_price: 9000,
            buy_price: buy,
            sell_price: sell,
            demand,
            stock,
            listed_at: Utc::now(),
        }
    }

    pub(crate) fn summary(name: &str, markets: i64) -> Summary {
        Summary {
            name: name.into(),
            markets,
            mean_price: 9000,
            lowest_buy: Some(8000),
            highest_sell: Some(10000),
        }
    }

    /// Three markets trading gold, one of each kind
    pub(crate) fn picked() -> Markets {
        let mut markets = Markets {
            commodities: vec![summary("gold", 3), summary("silver", 2)],
            ..Default::default()
        };
        markets.pick("gold");
        markets.quotes = vec![
            // Sells it and buys it.
            quote(1, "SOL", "Abraham Lincoln", 9500, 40, 9400, 300),
            // Only buys it.
            quote(2, "ALPHA CENTAURI", "Hutton Orbital", 0, 0, 9900, 12000),
            // Neither, having run out of it.
            quote(3, "LAVE", "Lave Station", 9100, 0, 0, 0),
        ];
        markets.reorder();
        markets
    }

    /// More markets trading gold than a window has room for
    fn many() -> Markets {
        let mut markets = Markets {
            commodities: vec![summary("gold", 200)],
            ..Default::default()
        };
        markets.pick("gold");
        markets.quotes = (0..200)
            .map(|i| {
                quote(
                    i,
                    &format!("SYSTEM {}", i),
                    &format!("Station {}", i),
                    9000 + i as i32,
                    40,
                    8900 + i as i32,
                    300,
                )
            })
            .collect();
        markets.reorder();
        markets
    }

    #[test]
    fn a_table_of_more_markets_than_will_fit_paints() {
        let mut markets = many();
        let quote = markets.quotes[0].clone();
        markets.open(&quote);
        markets.board = (0..200)
            .map(|i| commodity(0, &format!("thing{}", i), 900, 40, 800, 300))
            .collect();

        painted(|ui| {
            markets.draw(ui);
        });
    }

    #[test]
    fn the_three_panes_paint() {
        let mut markets = picked();
        let quote = markets.quotes[0].clone();
        markets.open(&quote);
        markets.board = vec![
            commodity(1, "gold", 9500, 40, 9400, 300),
            commodity(1, "silver", 0, 0, 700, 5000),
        ];

        painted(|ui| {
            markets.draw(ui);
        });
    }

    #[test]
    fn an_empty_window_paints() {
        let mut markets = Markets::default();
        painted(|ui| {
            markets.draw(ui);
        });
    }

    #[test]
    fn a_market_holding_none_of_it_says_so_rather_than_quoting_nothing() {
        let mut markets = picked();
        let said = words(|ui| {
            markets.draw(ui);
        });

        // Lave quotes 9,100 for gold it does not have, and Hutton pays 9,900
        // for gold it wants. Neither number the other lacks is drawn as a 0.
        assert!(said.iter().any(|word| word == "9,900"), "{:?}", said);
        assert!(!said.iter().any(|word| word == "0"), "{:?}", said);
    }

    #[test]
    fn the_commodity_list_is_narrowed_by_what_is_typed_over_it() {
        let mut markets = Markets {
            commodities: vec![summary("gold", 3), summary("silver", 2)],
            search: "sil".into(),
            ..Default::default()
        };

        let said = words(|ui| {
            markets.draw(ui);
        });
        assert!(said.iter().any(|word| word == "silver"), "{:?}", said);
        assert!(!said.iter().any(|word| word == "gold"), "{:?}", said);
    }

    #[test]
    fn picking_a_commodity_asks_where_it_trades() {
        let mut markets = Markets {
            commodities: vec![summary("gold", 3)],
            ..Default::default()
        };

        // Nothing is picked until something is clicked, so the pick is made
        // here and the ask it raises is what the click would have pushed.
        assert_eq!(markets.pick("gold"), Ask::Quotes("gold".into()));
        assert!(markets.quotes.is_empty());
    }

    #[test]
    fn a_price_reads_against_the_mean_only_where_it_is_worth_saying() {
        use crate::quotes::against;

        // Buying: under the mean is the good end of it.
        assert_eq!(against(Some(8000), Some(10000), true), Some(GOOD));
        assert_eq!(against(Some(12000), Some(10000), true), Some(BAD));
        // Selling: the other way round.
        assert_eq!(against(Some(12000), Some(10000), false), Some(GOOD));

        // A price a hair off the mean is every price, and coloring it says
        // nothing.
        assert_eq!(against(Some(10100), Some(10000), true), None);
        // A fleet carrier gives no mean price at all.
        assert_eq!(against(Some(10000), Some(0), true), None);
        assert_eq!(against(None, Some(10000), true), None);
    }

    #[test]
    fn thousands_marks_off_three_at_a_time() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(-12_345), "-12,345");
    }

    #[test]
    fn nothing_quoted_sorts_last_either_way_round() {
        let mut markets = picked();

        markets.sort = Sort { by: By::Sell, descending: true };
        markets.reorder();
        let order: Vec<_> = markets
            .shown()
            .iter()
            .map(|&i| markets.quotes[i].station_name.as_str())
            .collect();
        assert_eq!(
            order,
            ["Hutton Orbital", "Abraham Lincoln", "Lave Station"]
        );

        // Lave still has no sell price to be the least of, so it stays put
        // while the two real ones swap.
        markets.sort = Sort { by: By::Sell, descending: false };
        markets.reorder();
        let order: Vec<_> = markets
            .shown()
            .iter()
            .map(|&i| markets.quotes[i].station_name.as_str())
            .collect();
        assert_eq!(
            order,
            ["Abraham Lincoln", "Hutton Orbital", "Lave Station"]
        );
    }

    #[test]
    fn a_market_that_has_run_out_is_not_one_that_stocks_it() {
        let mut markets = picked();
        markets.only_stocked = true;
        markets.reorder();

        // Lave quotes a price for gold and holds none.
        let order: Vec<_> = markets
            .shown()
            .iter()
            .map(|&i| markets.quotes[i].station_name.as_str())
            .collect();
        assert_eq!(order, ["Abraham Lincoln"]);
    }

    #[test]
    fn narrowing_by_place_reads_either_name() {
        let mut markets = picked();

        markets.place = "hutton".into();
        markets.reorder();
        assert_eq!(markets.shown().len(), 1);

        markets.place = "sol".into();
        markets.reorder();
        assert_eq!(markets.shown().len(), 1);

        markets.place = "nowhere".into();
        markets.reorder();
        assert_eq!(markets.shown().len(), 0);
    }

    #[test]
    fn an_answer_to_a_question_since_moved_on_from_is_dropped() {
        let mut markets = picked();
        let quotes = markets.quotes.clone();
        markets.pick("silver");

        markets.take(Answer::Quotes("gold".into(), quotes.clone()));
        assert!(markets.quotes.is_empty());

        markets.take(Answer::Quotes("silver".into(), quotes));
        assert_eq!(markets.quotes.len(), 3);
    }
}
