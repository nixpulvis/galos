//! Everything one market trades
//!
//! The same rows the middle pane draws, read the other way round: one market
//! and all its commodities rather than one commodity and all its markets.
//! Picking a commodity here moves the other two panes to it, which is how a
//! station's board is used to find the next thing to carry.
use crate::quotes::{against, number};
use crate::trade::Endpoint;
use crate::{Ask, Markets, ROW, since, thousands};
use eframe::egui::{self, Ui};
use egui_extras::{Column, TableBuilder};

/// Draw the opened market's board, and any question picking a row raises
pub fn board(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    let Some(opened) = markets.opened.clone() else {
        return;
    };

    let mut shut = false;
    ui.horizontal(|ui| {
        ui.heading(&opened.station_name);
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                shut = ui.small_button("x").clicked();
            },
        );
    });

    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&opened.system_name).weak());
        // Every commodity a market lists is read at once, so the whole board
        // is as old as any row of it.
        if let Some(read) = markets.board.first().map(|c| c.listed_at) {
            ui.label(
                egui::RichText::new(format!("read {} ago", since(read))).weak(),
            );
        }
    });

    // Which end of a run this market is. Pinned from here because this is
    // where a market is already open and named.
    ui.horizontal(|ui| {
        let end = Endpoint {
            market_id: opened.market_id,
            system_name: opened.system_name.clone(),
            station_name: opened.station_name.clone(),
        };
        if ui
            .small_button("carry from here")
            .on_hover_text(
                "search for what this market sells, and where it is worth more",
            )
            .clicked()
        {
            asks.push(markets.pin_from(end.clone()));
        }
        if ui
            .small_button("to here")
            .on_hover_text("compare against wherever the run starts")
            .clicked()
        {
            asks.push(markets.pin_to(end));
        }
    });
    ui.add_space(4.);

    if markets.board.is_empty() {
        ui.weak("waiting on the database");
    } else {
        ui.label(
            egui::RichText::new(format!(
                "{} commodities",
                thousands(markets.board.len() as i64)
            ))
            .weak()
            .small(),
        );
    }
    ui.separator();

    let picked = markets.picked.clone();
    let mut pick = None;

    TableBuilder::new(ui)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::remainder().at_least(80.).clip(true))
        .columns(Column::initial(72.).at_least(48.), 4)
        .header(20., |mut header| {
            for label in ["Commodity", "Buy", "Stock", "Sell", "Demand"] {
                header.col(|ui| {
                    let text = egui::RichText::new(label).strong();
                    if label == "Commodity" {
                        ui.label(text);
                    } else {
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                ui.label(text);
                            },
                        );
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW, markets.board.len(), |mut row| {
                let commodity = &markets.board[row.index()];
                row.set_selected(picked.as_deref() == Some(&commodity.name));

                // What the market itself holds the thing to be worth, which
                // is what the game quotes beside its prices. A fleet carrier
                // gives no mean price at all, and those rows go uncolored.
                let mean = Some(commodity.mean_price);

                row.col(|ui| {
                    ui.label(&commodity.name);
                });
                row.col(|ui| {
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

                if row.response().clicked() {
                    pick = Some(commodity.name.clone());
                }
            });
        });

    if shut {
        markets.close();
    } else if let Some(name) = pick {
        asks.push(markets.pick(&name));
    }
}
