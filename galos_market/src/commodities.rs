//! Every commodity the galaxy trades
//!
//! The pane everything else is picked from. Nothing narrows this list but
//! the box over it: what is not traded anywhere is not in the table it is
//! read from, so there is nothing here to hide.
use crate::{Ask, Markets, thousands};
use eframe::egui::{self, Ui};

/// Draw the commodity list, and put any question picking one of them raises
///
/// Names are drawn as they are stored, which is lowercase and unseparated.
/// The game says "Fruit and Vegetables" where the data says
/// "fruitandvegetables", and nothing in the data says where one word ends.
pub fn commodities(ui: &mut Ui, markets: &mut Markets, asks: &mut Vec<Ask>) {
    ui.horizontal(|ui| {
        ui.heading("Commodities");
        ui.with_layout(
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                ui.label(
                    egui::RichText::new(thousands(
                        markets.commodities.len() as i64
                    ))
                    .weak(),
                );
            },
        );
    });

    ui.add(
        egui::TextEdit::singleline(&mut markets.search)
            .hint_text("filter")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(4.);

    let search = markets.search.trim().to_lowercase();
    let mut picked = None;

    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if markets.commodities.is_empty() {
            ui.weak("waiting on the database");
            return;
        }

        for summary in &markets.commodities {
            if !search.is_empty() && !summary.name.contains(&search) {
                continue;
            }

            let on = markets.picked.as_deref() == Some(&summary.name);
            ui.horizontal(|ui| {
                if ui.selectable_label(on, &summary.name).clicked() {
                    picked = Some(summary.name.clone());
                }
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.label(
                            egui::RichText::new(thousands(summary.markets))
                                .weak()
                                .small(),
                        );
                    },
                );
            });
        }
    });

    if let Some(name) = picked {
        asks.push(markets.pick(&name));
    }
}
