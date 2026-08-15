//! What is worth carrying, and where to
//!
//! One pane that asks a different question depending on how much has been
//! pinned. Nothing pinned is the galaxy board, which says what is worth money
//! somewhere. A source pinned is the useful one: what this market sells that
//! is worth more within reach. Both ends pinned is no search at all, just the
//! arithmetic of what each would pay the other.
//!
//! The pins are set from the board pane, which is where a market is already
//! open and named.
use crate::trade::{Comparison, Trade};
use crate::{Ask, BAD, GOOD, Markets, ROW, Rank, since, thousands};
use eframe::egui::{self, Response, Ui};
use egui_extras::{Column, TableBuilder};

/// Whether a number has finished being changed
///
/// A [`egui::DragValue`] is usually dragged, which never takes focus, and is
/// sometimes typed into, which does. Asking on every change would put a
/// second-long query behind every pixel of a drag, so both endings are waited
/// for instead.
trait Settled {
    fn settled(&self) -> bool;
}

impl Settled for Response {
    fn settled(&self) -> bool {
        self.drag_stopped() || self.lost_focus()
    }
}

/// Draw whichever question the pins have asked for
pub fn trades(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    pins(ui, markets, asks);
    knobs(ui, markets, asks);
    ui.separator();

    if markets.from.is_some() && markets.to.is_some() {
        comparison(ui, markets);
    } else {
        found(ui, markets, asks);
    }
}

/// What is pinned, and what that makes the pane ask
fn pins(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    let mut changed = false;

    let anchored = markets.near.trim().is_empty();
    ui.horizontal(|ui| {
        ui.heading(match (&markets.from, &markets.to) {
            (Some(_), Some(_)) => "Between",
            (Some(_), None) => "From here",
            _ if !anchored => "Near",
            _ => "Anywhere",
        });

        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if ui.button("Search").clicked() {
                    changed = true;
                }
            },
        );
    });

    for (slot, label) in [(0, "from"), (1, "to")] {
        let pinned = if slot == 0 { &markets.from } else { &markets.to };
        let Some(end) = pinned.clone() else { continue };

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).weak().small());
            ui.label(format!("{} / {}", end.system_name, end.station_name));
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui.small_button("x").clicked() {
                        if slot == 0 {
                            markets.from = None;
                        } else {
                            markets.to = None;
                        }
                        changed = true;
                    }
                },
            );
        });
    }

    // Somewhere to stand, where no market has been pinned. A pinned market
    // is the same question asked more precisely, so the box goes away rather
    // than sitting there meaning nothing.
    if markets.from.is_none() {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("near").weak().small());
            let box_ = ui.add(
                egui::TextEdit::singleline(&mut markets.near)
                    .hint_text("a system you are at")
                    .desired_width(160.),
            );
            // Typing a system name a letter at a time is not a search a
            // letter at a time. Return, or clicking off the box, is.
            changed |= box_.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter));
        });

        ui.label(
            egui::RichText::new(if markets.near.trim().is_empty() {
                "the biggest margins on record, however far apart their ends"
            } else {
                "both ends within reach of there, so the whole run is local"
            })
            .weak()
            .small(),
        );
    }

    if changed {
        asks.push(markets.search());
    }
}

/// The knobs that decide what counts as a trade
fn knobs(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    let mut changed = false;
    let filters = &mut markets.filters;

    ui.horizontal(|ui| {
        ui.label("hold");
        // Only ever divides what is already fetched, so it asks nothing.
        ui.add(egui::DragValue::new(&mut filters.hold).range(1..=2000));

        ui.label("stock");
        changed |= ui
            .add(
                egui::DragValue::new(&mut filters.min_stock).range(0..=100_000),
            )
            .settled();
        ui.label("demand");
        changed |= ui
            .add(
                egui::DragValue::new(&mut filters.min_demand)
                    .range(0..=100_000),
            )
            .settled();
    });

    ui.horizontal(|ui| {
        ui.label("days");
        changed |= ui
            .add(egui::DragValue::new(&mut filters.max_age).range(1..=90))
            .settled();

        // Only means anything with somewhere to measure from, which is a
        // pinned market or a named system. The galaxy board has neither, and
        // says so by grey.
        let anchored =
            markets.from.is_some() || !markets.near.trim().is_empty();
        ui.add_enabled_ui(anchored, |ui| {
            let mut bounded = filters.within.is_some();
            if ui.checkbox(&mut bounded, "within").changed() {
                filters.within = bounded.then_some(100.);
                changed = true;
            }
            if let Some(within) = &mut filters.within {
                changed |= ui
                    .add(
                        egui::DragValue::new(within)
                            .range(1.0..=1000.)
                            .suffix(" ly"),
                    )
                    .settled();
            }
        });

        changed |= ui.checkbox(&mut filters.carriers, "carriers").changed();
    });

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("order by").weak().small());
        for (rank, label) in [
            (Rank::Margin, "margin"),
            (Rank::Haul, "run"),
            (Rank::Distance, "distance"),
        ] {
            if ui.selectable_label(markets.rank == rank, label).clicked() {
                markets.rank = rank;
                markets.reorder_trades();
            }
        }
    });

    if changed {
        asks.push(markets.search());
    }
}

/// The trades a search turned up
fn found(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    if markets.trades.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.weak("nothing found, or nothing asked for yet");
        });
        return;
    }

    let hold = markets.filters.hold;
    let anchored = markets.from.is_some();
    let mut pin = None;

    TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::initial(150.).at_least(80.).clip(true))
        .columns(Column::initial(74.).at_least(48.), 3)
        .column(Column::initial(92.).at_least(56.))
        .column(Column::initial(64.).at_least(44.))
        .column(Column::remainder().at_least(120.).clip(true))
        .header(20., |mut header| {
            for (label, right) in [
                ("Commodity", false),
                ("Buy", true),
                ("Sell", true),
                ("Margin", true),
                ("Run", true),
                ("Distance", true),
                (if anchored { "To" } else { "From -> To" }, false),
            ] {
                header.col(|ui| {
                    let text = egui::RichText::new(label).strong();
                    if right {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(text);
                            },
                        );
                    } else {
                        ui.label(text);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW, markets.trades.len(), |mut row| {
                let trade = &markets.trades[row.index()];

                row.col(|ui| {
                    ui.label(&trade.name);
                });
                row.col(|ui| number(ui, trade.buy_price as i64, None));
                row.col(|ui| number(ui, trade.sell_price as i64, None));
                row.col(|ui| {
                    number(ui, trade.margin() as i64, Some(GOOD));
                });
                row.col(|ui| {
                    // What the run is actually worth, which is where a
                    // spectacular price on four tons of something stops
                    // looking like a fortune.
                    let earned = trade.haul(hold);
                    number(ui, earned, (earned > 0).then_some(GOOD));
                });
                row.col(|ui| {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| match trade.distance {
                            Some(ly) => {
                                // Far enough that the margin is theoretical.
                                let color = (ly > 500.).then_some(BAD);
                                let mut text = egui::RichText::new(format!(
                                    "{:.0} ly",
                                    ly
                                ))
                                .monospace();
                                if let Some(color) = color {
                                    text = text.color(color);
                                }
                                ui.label(text);
                            }
                            // One end is a market still waiting on the system
                            // it named, so there is nothing to measure.
                            None => {
                                ui.label(
                                    egui::RichText::new("?").weak().monospace(),
                                );
                            }
                        },
                    );
                });
                row.col(|ui| {
                    let said = if anchored {
                        format!(
                            "{} / {}",
                            trade.destination.system_name,
                            trade.destination.station_name
                        )
                    } else {
                        format!(
                            "{} -> {}",
                            trade.source.station_name,
                            trade.destination.station_name
                        )
                    };
                    ui.label(egui::RichText::new(said).weak());
                });

                if row.response().clicked() {
                    pin = Some(row.index());
                }
            });
        });

    // Clicking a row pins its two ends, which turns the search into the
    // comparison of the pair it just proposed.
    if let Some(index) = pin {
        let trade = markets.trades[index].clone();
        asks.push(markets.pin(trade.source, trade.destination));
    }
}

/// Everything two pinned markets both trade
fn comparison(ui: &mut Ui, markets: &mut Markets) {
    ui.horizontal(|ui| {
        match markets.apart {
            Some(ly) => ui.label(format!("{:.0} ly apart", ly)),
            None => ui.label(
                egui::RichText::new("one of them has no system on record")
                    .weak(),
            ),
        };
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{} in common",
                        thousands(markets.comparison.len() as i64)
                    ))
                    .weak(),
                );
            },
        );
    });
    ui.add_space(4.);

    if markets.comparison.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.weak("nothing both of them carry");
        });
        return;
    }

    let hold = markets.filters.hold;

    TableBuilder::new(ui)
        .striped(true)
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(100.).clip(true))
        .columns(Column::initial(76.).at_least(48.), 6)
        .header(20., |mut header| {
            for (label, right) in [
                ("Commodity", false),
                ("Buy", true),
                ("Stock", true),
                ("Sell", true),
                ("Demand", true),
                ("Out", true),
                ("Back", true),
            ] {
                header.col(|ui| {
                    let text = egui::RichText::new(label).strong();
                    if right {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(text);
                            },
                        );
                    } else {
                        ui.label(text);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW, markets.comparison.len(), |mut row| {
                let row_index = row.index();
                let pair: &Comparison = &markets.comparison[row_index];

                row.col(|ui| {
                    ui.label(&pair.name);
                });
                // What the near end sells it for, and what the far end pays.
                // The two columns either side of the middle are the two
                // halves of the run out.
                row.col(|ui| {
                    maybe(
                        ui,
                        pair.here
                            .is_stocked()
                            .then_some(pair.here.buy_price as i64),
                    );
                });
                row.col(|ui| {
                    maybe(
                        ui,
                        pair.here
                            .is_stocked()
                            .then_some(pair.here.stock as i64),
                    );
                });
                row.col(|ui| {
                    maybe(
                        ui,
                        pair.there
                            .is_wanted()
                            .then_some(pair.there.sell_price as i64),
                    );
                });
                row.col(|ui| {
                    maybe(
                        ui,
                        pair.there
                            .is_wanted()
                            .then_some(pair.there.demand as i64),
                    );
                });
                row.col(|ui| profit(ui, pair.out(), pair, hold, true));
                row.col(|ui| profit(ui, pair.back(), pair, hold, false));
            });
        });
}

/// A margin, in the color of which way it goes
///
/// A loss is a real answer and worth drawing. It says the run is worth making
/// the other way, which is the column beside it.
fn profit(
    ui: &mut Ui,
    margin: Option<i32>,
    pair: &Comparison,
    hold: i32,
    out: bool,
) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        match margin {
            Some(margin) => {
                let tons = if out {
                    pair.here.stock.min(pair.there.demand)
                } else {
                    pair.there.stock.min(pair.here.demand)
                }
                .min(hold);
                let text =
                    egui::RichText::new(thousands(margin as i64 * tons as i64))
                        .monospace()
                        .color(if margin > 0 { GOOD } else { BAD });
                ui.label(text)
                    .on_hover_text(format!("{} a ton, {} tons", margin, tons));
            }
            None => {
                ui.label(egui::RichText::new("-").weak().monospace());
            }
        }
    });
}

/// A number, or a dash where there is none
fn maybe(ui: &mut Ui, value: Option<i64>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        match value {
            Some(value) => {
                ui.label(egui::RichText::new(thousands(value)).monospace());
            }
            None => {
                ui.label(egui::RichText::new("-").weak().monospace());
            }
        }
    });
}

/// A number, right aligned like the rest of them
fn number(ui: &mut Ui, value: i64, color: Option<egui::Color32>) {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let mut text = egui::RichText::new(thousands(value)).monospace();
        if let Some(color) = color {
            text = text.color(color);
        }
        ui.label(text);
    });
}

/// How stale the trades on screen are
pub fn staleness(trades: &[Trade]) -> Option<String> {
    trades.iter().map(|t| t.read_at).min().map(|at| since(at))
}
