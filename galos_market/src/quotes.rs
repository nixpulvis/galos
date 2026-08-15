//! Everywhere one commodity trades
//!
//! A row for every market carrying it, whether or not it has any to sell or
//! any room to buy. What a market quotes while holding none of something is
//! still what that market thinks the thing is worth, and dropping those rows
//! would leave a station that has run out looking like one that never
//! carried it. The two boxes over the table are how they are put away.
use crate::{Ask, BAD, By, GOOD, Markets, ROW, Sort, since, thousands};
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};

/// How far off the mean a price has to be before it is worth coloring
///
/// Every market is a little off it. Coloring that says nothing and leaves a
/// table where each column is half green and half red, which is a harder
/// thing to read than no color at all.
const NOTABLE: f32 = 0.1;

/// Draw everywhere the picked commodity trades
pub fn quotes(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    let Some(picked) = markets.picked.clone() else {
        ui.centered_and_justified(|ui| {
            ui.weak("pick a commodity");
        });
        return;
    };

    header(ui, markets, &picked);
    ui.separator();
    table(ui, markets, asks);
}

/// What the galaxy makes of the commodity, and the boxes narrowing the table
fn header(ui: &mut Ui, markets: &mut Markets, picked: &str) {
    ui.horizontal(|ui| {
        ui.heading(picked);
        if let Some(summary) = markets.summary() {
            let mut said =
                format!("{} markets", thousands(summary.markets as i64));
            said +=
                &format!(", mean {} cr", thousands(summary.mean_price as i64));
            if let Some(lowest) = summary.lowest_buy {
                said += &format!(", from {} cr", thousands(lowest as i64));
            }
            if let Some(highest) = summary.highest_sell {
                said += &format!(", up to {} cr", thousands(highest as i64));
            }
            ui.label(egui::RichText::new(said).weak());
        }
    });

    ui.horizontal(|ui| {
        let mut changed = false;
        changed |= ui.checkbox(&mut markets.only_stocked, "in stock").changed();
        changed |= ui.checkbox(&mut markets.only_wanted, "wanted").changed();
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut markets.place)
                    .hint_text("system or station")
                    .desired_width(160.),
            )
            .changed();

        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let shown = markets.shown().len();
                let held = markets.quotes.len();
                let said = if shown == held {
                    format!("{} markets", thousands(shown as i64))
                } else {
                    format!(
                        "{} of {} markets",
                        thousands(shown as i64),
                        thousands(held as i64)
                    )
                };
                ui.label(egui::RichText::new(said).weak());
            },
        );

        if changed {
            markets.reorder();
        }
    });
}

/// The table itself
fn table(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    // Both are read inside the closures the table lays out in, which hold
    // `markets` for as long as they run. Picking them out here is what lets
    // a click inside change either one.
    let sort = markets.sort;
    let mean = markets.summary().map(|s| s.mean_price);
    let opened = markets.opened.as_ref().map(|o| o.market_id);

    let mut resort = None;
    let mut clicked = None;

    TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(150.).at_least(80.).clip(true))
        .column(Column::remainder().at_least(100.).clip(true))
        .columns(Column::initial(84.).at_least(52.), 4)
        .column(Column::initial(56.).at_least(40.))
        .header(20., |mut header| {
            header
                .col(|ui| column(ui, "System", By::System, sort, &mut resort));
            header.col(|ui| {
                column(ui, "Station", By::Station, sort, &mut resort)
            });
            header.col(|ui| right(ui, "Buy", By::Buy, sort, &mut resort));
            header.col(|ui| right(ui, "Stock", By::Stock, sort, &mut resort));
            header.col(|ui| right(ui, "Sell", By::Sell, sort, &mut resort));
            header.col(|ui| right(ui, "Demand", By::Demand, sort, &mut resort));
            header.col(|ui| right(ui, "Read", By::Updated, sort, &mut resort));
        })
        .body(|body| {
            body.rows(ROW, markets.shown().len(), |mut row| {
                let quote = &markets.quotes[markets.shown()[row.index()]];
                let commodity = &quote.commodity;
                row.set_selected(opened == Some(commodity.market_id));

                row.col(|ui| {
                    ui.label(&quote.system_name);
                });
                row.col(|ui| {
                    ui.label(&quote.station_name);
                });
                row.col(|ui| {
                    // Cheap is good news for the side of the trade a buy
                    // price is on, and dear is good news for the other.
                    let price =
                        commodity.is_stocked().then_some(commodity.buy_price);
                    number(ui, price, against(price, mean, true));
                });
                row.col(|ui| {
                    number(
                        ui,
                        commodity.is_stocked().then_some(commodity.stock),
                        None,
                    );
                });
                row.col(|ui| {
                    let price =
                        commodity.is_wanted().then_some(commodity.sell_price);
                    number(ui, price, against(price, mean, false));
                });
                row.col(|ui| {
                    number(
                        ui,
                        commodity.is_wanted().then_some(commodity.demand),
                        None,
                    );
                });
                row.col(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            ui.label(
                                egui::RichText::new(since(commodity.listed_at))
                                    .weak(),
                            );
                        },
                    );
                });

                if row.response().clicked() {
                    clicked = Some(markets.shown()[row.index()]);
                }
            });
        });

    if let Some(by) = resort {
        // Asking for the column already sorted by is asking for it the other
        // way round, which is the only way to say so with one gesture.
        markets.sort = if sort.by == by {
            Sort { by, descending: !sort.descending }
        } else {
            Sort { by, descending: sort.descending }
        };
        markets.reorder();
    }

    if let Some(index) = clicked {
        let quote = markets.quotes[index].clone();
        asks.push(markets.open(&quote));
    }
}

/// A heading that sorts the table by its column
fn column(
    ui: &mut Ui,
    label: &str,
    by: By,
    sort: Sort,
    resort: &mut Option<By>,
) {
    let arrow = match (sort.by == by, sort.descending) {
        (true, true) => " v",
        (true, false) => " ^",
        (false, _) => "",
    };
    let text = egui::RichText::new(format!("{}{}", label, arrow)).strong();
    if ui.selectable_label(sort.by == by, text).clicked() {
        *resort = Some(by);
    }
}

/// The same, over a column of numbers, which are read from the right
fn right(
    ui: &mut Ui,
    label: &str,
    by: By,
    sort: Sort,
    resort: &mut Option<By>,
) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        column(ui, label, by, sort, resort);
    });
}

/// Draw a number where there is one, and say so where there is not
///
/// A market holding none of something quotes a price for it anyway. Drawing
/// that as a zero would put it at one end of a sorted column among real
/// numbers, so it is drawn as nothing at all.
pub(crate) fn number(
    ui: &mut Ui,
    value: Option<i32>,
    color: Option<egui::Color32>,
) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        match value {
            Some(value) => {
                let mut text =
                    egui::RichText::new(thousands(value as i64)).monospace();
                if let Some(color) = color {
                    text = text.color(color);
                }
                ui.label(text);
            }
            None => {
                ui.label(egui::RichText::new("-").weak().monospace());
            }
        }
    });
}

/// How a price reads against the galactic mean, where it is worth saying
///
/// `cheap_is_good` is which side of the trade the price is on: a low buy
/// price is a bargain, and a low sell price is a poor one.
pub(crate) fn against(
    price: Option<i32>,
    mean: Option<i32>,
    cheap_is_good: bool,
) -> Option<egui::Color32> {
    let (price, mean) = (price?, mean?);
    if mean <= 0 {
        return None;
    }

    let off = (price - mean) as f32 / mean as f32;
    if off.abs() < NOTABLE {
        return None;
    }
    Some(if (off < 0.) == cheap_is_good { GOOD } else { BAD })
}
