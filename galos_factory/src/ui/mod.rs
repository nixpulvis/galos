//! Shared egui panels for the production sim. Added by both the standalone
//! runner and the full game (`galos_game`), so every dashboard built here
//! is available in both. Panels never mutate sim state directly — they push
//! [`PlayerCommand`]s, keeping every action tick-aligned.

use crate::data::{BuildingKind, StaticData};
use crate::sim::commands::PlayerCommand;
use crate::sim::*;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

pub fn ui_plugin(app: &mut App) {
    app.init_resource::<Selected>();
    app.add_systems(
        Update,
        (status_bar, stations_panel, logistics_panel, ticker_panel),
    );
}

/// UI selection state (which station's detail view is open).
#[derive(Resource, Default)]
pub struct Selected {
    pub station: Option<Entity>,
}

fn speed_label(speed: SimSpeed) -> &'static str {
    match speed {
        SimSpeed::Paused => "⏸",
        SimSpeed::X1 => "1x",
        SimSpeed::X10 => "10x",
        SimSpeed::X60 => "60x",
    }
}

fn status_label(status: FactoryStatus) -> &'static str {
    match status {
        FactoryStatus::Idle => "idle",
        FactoryStatus::Running => "running",
        FactoryStatus::Starved => "STARVED",
        FactoryStatus::OutputBlocked => "BLOCKED",
        FactoryStatus::Offline => "OFFLINE",
    }
}

fn status_bar(
    mut contexts: EguiContexts,
    clock: Res<SimClock>,
    credits: Res<Credits>,
    speed: Res<SimSpeed>,
    mut queue: ResMut<CommandQueue>,
) {
    egui::TopBottomPanel::top("status").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("tick {}", clock.tick));
            ui.separator();
            ui.label(format!("{} cr", credits.0));
            ui.separator();
            for s in
                [SimSpeed::Paused, SimSpeed::X1, SimSpeed::X10, SimSpeed::X60]
            {
                if ui.selectable_label(*speed == s, speed_label(s)).clicked() {
                    queue.0.push(PlayerCommand::SetSpeed(s));
                }
            }
        });
    });
}

fn stations_panel(
    mut contexts: EguiContexts,
    data: Res<StaticData>,
    mut selected: ResMut<Selected>,
    mut queue: ResMut<CommandQueue>,
    stations: Query<(
        Entity,
        &Station,
        &Storage,
        &PowerGrid,
        Option<&LifeSupport>,
        &Slots,
    )>,
    factories: Query<(Entity, &Factory, &ActiveRecipe, &Status)>,
) {
    egui::Window::new("Stations").default_width(380.0).show(
        contexts.ctx_mut(),
        |ui| {
            for (entity, station, storage, grid, life, slots) in stations.iter()
            {
                let owner = match station.owner {
                    Owner::Player => "you",
                    Owner::Npc => "npc",
                };
                let header = format!(
                    "{} [{owner}]  {}/{} stored  {}MW/{}MW",
                    station.name,
                    storage.total(),
                    storage.cap,
                    grid.supply_mw,
                    grid.demand_mw,
                );
                let open = selected.station == Some(entity);
                if ui.selectable_label(open, header).clicked() {
                    selected.station = if open { None } else { Some(entity) };
                }
                if selected.station != Some(entity) {
                    continue;
                }

                ui.indent(entity, |ui| {
                    if let Some(life) = life {
                        if !life.life_support_ok {
                            ui.colored_label(
                                egui::Color32::RED,
                                "LIFE SUPPORT SHORT",
                            );
                        }
                    }

                    let mine: Vec<_> = factories
                        .iter()
                        .filter(|(_, f, _, _)| f.station == entity)
                        .collect();
                    ui.label(format!("slots {}/{}", mine.len(), slots.total));

                    for (factory_entity, factory, active, status) in &mine {
                        ui.horizontal(|ui| {
                            ui.label(format!("{:?}", factory.kind));
                            let current = active
                                .0
                                .map(|id| data.recipe(id).id.as_str())
                                .unwrap_or("-");
                            egui::ComboBox::from_id_source(*factory_entity)
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    for (recipe_id, recipe) in
                                        data.recipes_for(factory.kind)
                                    {
                                        if ui
                                            .selectable_label(
                                                active.0 == Some(recipe_id),
                                                &recipe.id,
                                            )
                                            .clicked()
                                        {
                                            queue.0.push(
                                                PlayerCommand::SetRecipe {
                                                    factory: *factory_entity,
                                                    recipe: Some(recipe_id),
                                                    output_cap: None,
                                                },
                                            );
                                        }
                                    }
                                });
                            ui.label(status_label(status.0));
                        });
                    }

                    if station.owner == Owner::Player {
                        ui.menu_button("build…", |ui| {
                            for kind in [
                                BuildingKind::Extractor,
                                BuildingKind::FuelScoop,
                                BuildingKind::Refinery,
                                BuildingKind::Assembler,
                                BuildingKind::PowerPlant,
                                BuildingKind::SolarArray,
                                BuildingKind::Geothermal,
                                BuildingKind::StorageModule,
                            ] {
                                let def = data.building(kind);
                                let cost: Vec<String> = def
                                    .cost
                                    .iter()
                                    .map(|(item, qty)| {
                                        format!("{qty} {}", data.item(*item).id)
                                    })
                                    .collect();
                                let label = format!(
                                    "{kind:?} ({}, {} cr)",
                                    cost.join(" + "),
                                    def.credits_cost
                                );
                                if ui.button(label).clicked() {
                                    queue.0.push(PlayerCommand::Build {
                                        station: entity,
                                        kind,
                                    });
                                    ui.close_menu();
                                }
                            }
                        });
                    }

                    egui::CollapsingHeader::new("storage")
                        .id_source((entity, "storage"))
                        .show(ui, |ui| {
                            let mut inventory: Vec<_> =
                                storage.pool.iter().collect();
                            inventory.sort_by_key(|(item, _)| item.0);
                            for (item, qty) in inventory {
                                ui.label(format!(
                                    "{:<24} {qty}",
                                    data.item(*item).name
                                ));
                            }
                        });
                });
            }
        },
    );
}

fn logistics_panel(
    mut contexts: EguiContexts,
    data: Res<StaticData>,
    contracts: Query<(Entity, &Contract)>,
    ships: Query<&Ship>,
    stations: Query<&Station>,
) {
    egui::Window::new("Logistics").default_width(340.0).show(
        contexts.ctx_mut(),
        |ui| {
            let name = |entity: Entity| {
                stations
                    .get(entity)
                    .map(|s| s.name.as_str())
                    .unwrap_or("?")
                    .to_string()
            };
            for (contract_entity, contract) in contracts.iter() {
                let fleet = ships
                    .iter()
                    .filter(|s| s.contract == Some(contract_entity))
                    .count();
                ui.label(format!(
                    "{} : {} → {}  ({} ship{})",
                    data.item(contract.item).name,
                    name(contract.from),
                    name(contract.to),
                    fleet,
                    if fleet == 1 { "" } else { "s" },
                ));
                ui.indent(contract_entity, |ui| {
                    for ship in ships
                        .iter()
                        .filter(|s| s.contract == Some(contract_entity))
                    {
                        let state = match &ship.state {
                            ShipState::Idle { .. } => "idle".to_string(),
                            ShipState::Loading => "loading".to_string(),
                            ShipState::Outbound { ticks_left, cargo } => {
                                format!(
                                    "outbound ({cargo} units, {ticks_left}t)"
                                )
                            }
                            ShipState::Returning { ticks_left } => {
                                format!("returning ({ticks_left}t)")
                            }
                        };
                        ui.label(format!("{:?}: {state}", ship.class));
                    }
                });
            }
        },
    );
}

fn ticker_panel(
    mut contexts: EguiContexts,
    data: Res<StaticData>,
    notices: Res<Notices>,
) {
    egui::Window::new("Ticker").default_width(360.0).show(
        contexts.ctx_mut(),
        |ui| {
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for (tick, notice) in notices.0.iter().rev().take(40) {
                    let text = match notice {
                        Notice::Built { station, kind } => {
                            format!("built {kind:?} at {station}")
                        }
                        Notice::BuildFailed { station, reason } => {
                            format!("build failed at {station}: {reason}")
                        }
                        Notice::Brownout { station } => {
                            format!("brownout at {station}")
                        }
                        Notice::LifeSupportShort { station } => {
                            format!("life support short at {station}")
                        }
                        Notice::MaintenanceShort { station, kind } => {
                            format!("{kind:?} maintenance unpaid at {station}")
                        }
                        Notice::NoFuel { station } => {
                            format!("no fuel at {station}")
                        }
                        Notice::PiracyLoss { item, qty } => {
                            format!(
                                "PIRACY: lost {qty} {}",
                                data.item(*item).name
                            )
                        }
                        Notice::Sold { station, item, qty, credits } => {
                            format!(
                                "sold {qty} {} at {station} for {credits} cr",
                                data.item(*item).name
                            )
                        }
                        Notice::Bought { station, item, qty, credits } => {
                            format!(
                                "bought {qty} {} to {station} for {credits} cr",
                                data.item(*item).name
                            )
                        }
                    };
                    ui.label(format!("[{tick}] {text}"));
                }
            });
        },
    );
}
