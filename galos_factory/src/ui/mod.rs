//! Shared egui panels for the production sim. Added by both the standalone
//! runner and the full game (`galos_game`), so every dashboard built here
//! is available in both. Panels never mutate sim state directly — they send
//! [`PlayerCommand`] events on behalf of [`LocalCommander`], keeping every
//! action tick-aligned and attributable.

use crate::data::{BuildingKind, StaticData};
use crate::sim::*;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

pub fn ui_plugin(app: &mut App) {
    app.init_resource::<Selected>();
    app.add_systems(
        Update,
        (status_bar, stations_panel, logistics_panel, ticker_panel)
            .run_if(resource_exists::<LocalCommander>),
    );
}

/// The commander this client is playing. In a shared world every other
/// commander's assets are visible but not actionable.
#[derive(Resource, Clone, Copy, Debug)]
pub struct LocalCommander(pub Entity);

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
    speed: Res<SimSpeed>,
    me: Res<LocalCommander>,
    commanders: Query<(&Commander, &Credits)>,
    mut commands: EventWriter<PlayerCommand>,
) {
    egui::TopBottomPanel::top("status").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            if let Ok((commander, credits)) = commanders.get(me.0) {
                ui.label(&commander.name);
                ui.separator();
                ui.label(format!("{} cr", credits.0));
                ui.separator();
            }
            ui.label(format!("tick {}", clock.tick));
            ui.separator();
            for s in
                [SimSpeed::Paused, SimSpeed::X1, SimSpeed::X10, SimSpeed::X60]
            {
                if ui.selectable_label(*speed == s, speed_label(s)).clicked() {
                    commands
                        .send(PlayerCommand::new(me.0, Action::SetSpeed(s)));
                }
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn stations_panel(
    mut contexts: EguiContexts,
    data: Res<StaticData>,
    me: Res<LocalCommander>,
    mut selected: ResMut<Selected>,
    mut commands: EventWriter<PlayerCommand>,
    stations: Query<(
        Entity,
        &Station,
        &OwnedBy,
        &Storage,
        &PowerGrid,
        &LifeSupport,
        &Slots,
        Option<&Children>,
    )>,
    owners: Query<AnyOf<(&Commander, &Faction)>>,
    factories: Query<(&Factory, &ActiveRecipe, &Status)>,
) {
    egui::Window::new("Stations").default_width(380.0).show(
        contexts.ctx_mut(),
        |ui| {
            for (
                entity,
                station,
                owner,
                storage,
                grid,
                life,
                slots,
                children,
            ) in stations.iter()
            {
                let mine = owner.0 == me.0;
                let owner_name = match owners.get(owner.0) {
                    Ok((Some(commander), _)) => commander.name.clone(),
                    Ok((_, Some(faction))) => faction.name.clone(),
                    _ => "unknown".into(),
                };
                let header = format!(
                    "{} [{owner_name}]  {}/{} stored  {}MW/{}MW",
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
                    if !life.ok {
                        ui.colored_label(
                            egui::Color32::RED,
                            "LIFE SUPPORT SHORT",
                        );
                    }
                    let kids = children.map(|c| &**c).unwrap_or(&[]);
                    ui.label(format!("slots {}/{}", kids.len(), slots.total));

                    for &child in kids {
                        let Ok((factory, active, status)) =
                            factories.get(child)
                        else {
                            continue;
                        };
                        ui.horizontal(|ui| {
                            ui.label(format!("{:?}", factory.kind));
                            let current = active
                                .0
                                .map(|id| data.recipe(id).id.as_str())
                                .unwrap_or("-");
                            ui.add_enabled_ui(mine, |ui| {
                                egui::ComboBox::from_id_source(child)
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
                                                commands.send(
                                                    PlayerCommand::new(
                                                        me.0,
                                                        Action::SetRecipe {
                                                            factory: child,
                                                            recipe: Some(
                                                                recipe_id,
                                                            ),
                                                            output_cap: None,
                                                        },
                                                    ),
                                                );
                                            }
                                        }
                                    });
                            });
                            ui.label(status_label(status.0));
                        });
                    }

                    if mine {
                        ui.menu_button("build…", |ui| {
                            for kind in BuildingKind::ALL {
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
                                    commands.send(PlayerCommand::new(
                                        me.0,
                                        Action::Build { station: entity, kind },
                                    ));
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
    me: Res<LocalCommander>,
    contracts: Query<(Entity, &Contract, &OwnedBy)>,
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
            for (contract_entity, contract, owner) in contracts.iter() {
                if owner.0 != me.0 {
                    continue;
                }
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
                for (tick, notice) in notices.recent(40) {
                    let text = match notice {
                        Notice::Built { station, kind } => {
                            format!("built {kind:?} at {station}")
                        }
                        Notice::CommandRejected { reason } => {
                            format!("rejected: {reason}")
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
                        Notice::PiracyLoss { item, qty } => format!(
                            "PIRACY: lost {qty} {}",
                            data.item(*item).name
                        ),
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
