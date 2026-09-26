//! A filter's panel: the systems it admits, and for a route what the flying
//! comes to

use crate::map::camera::MoveCamera;
use crate::map::filter::{Filter, Filters, Plotted};
use crate::map::galaxy::System;
use crate::ui::MARGIN;
use crate::ui::list::SystemAction;
use crate::ui::panels::fuel::{Scooping, StarClasses, fuel_rule};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;
use galos_route::graph::Crossing;
use std::time::Duration;

/// What a filter's panel says it is showing, above the list of it
///
/// How many systems, and for a route the whole of what it was plotted for: the
/// range, the drive, and how hard the search worked at it. A panel is titled
/// with what the filter is called, and a route is called after its two ends,
/// so two plots between the same pair come up under the same name. What tells
/// them apart is what was asked for — and all three of those are part of the
/// ask, each of them able to change which systems come back.
///
/// A range rather than a jump. It is how far the ship can go in one, which is
/// what the route was worked out against; how far it actually goes is the
/// distance on each line of the list below, and is usually less. Which is the
/// reason the drive belongs here beside it: a jump of nine hundred light years
/// on a line below, under a range of a hundred and fifty, is a jet cone and
/// not a mistake, and nothing else on screen said a jet cone was allowed.
///
/// Said in the panel rather than in the title, which is cut to the room a
/// window has and would lose it. The other filters are named for the whole of
/// what they are and have nothing to add here.
fn summary(
    filter: &Filter,
    count: usize,
    took: Option<Duration>,
    plotting: usize,
    plotted: Option<Plotted>,
) -> String {
    let Some(range) = filter.range() else {
        return format!("{count} systems");
    };
    // Both are part of what was asked, and both are read off the route rather
    // than off the settings, which the user may have moved since.
    let boosted = filter.drive().and_then(|drive| drive.named());
    // What was asked for, in its own words: "fewest jumps", or "within 5%
    // of fewest", with ", shortest" where the ties were settled by
    // distance. It used to be one of three mode names and a trailing
    // "search", which said which button was pressed rather than what the
    // route is.
    let how = filter.how().map(|how| how.named()).unwrap_or_default();
    let mut said = match boosted {
        Some(boosted) => {
            format!("{count} systems, {range} Ly range, {boosted}, {how}")
        }
        None => format!("{count} systems, {range} Ly range, {how}"),
    };
    // What it cost to find, which is the other half of what the search mode
    // means: `quick` and `direct` differ by a jump or two and by minutes,
    // and the row that says which was asked for should say what it came to.
    // Nothing for a route this session did not plot — one restored, or one
    // whose row has been closed and re-added.
    if let Some(took) = took {
        said.push_str(&format!(
            ", plotted in {}",
            crate::ui::text::waited(took)
        ));
    }
    // And what is *not* in it yet. A trip is plotted a leg at a time, so a
    // panel read before the last of them lands describes a real route
    // through some of the stops — which is not the trip it is titled after,
    // and saying nothing about that is the panel lying by omission. The
    // systems, the distance and the longest jump below are all of them
    // about what has landed.
    if plotting > 0 {
        let legs = match plotting {
            1 => "1 leg".to_owned(),
            legs => format!("{legs} legs"),
        };
        said.push_str(&format!(" — {legs} still being plotted"));
    }
    // And what became of this row's own search, where it never landed a
    // route. Its systems are then the two ends it was asked between rather
    // than a route through them, so a panel reading "2 systems, 45 Ly range"
    // over them claims an answer nobody has: a leg still being walked, one
    // stopped, and one with no route to be found each say which.
    if let Some(ended) =
        plotted.filter(|how| !how.landed()).and_then(Plotted::said)
    {
        said.push_str(&format!(" — {ended}"));
    }
    said
}

/// How the plan behind a route was worked out, where there was one
///
/// The other half of what a route was asked for. The line above says the
/// range, the drive, the optimality and what the search cost; these are
/// the settings that decide how the coarse plan over the boost stars went
/// about it, and they are part of a route's own identity
/// ([`Filter::Route`]) precisely because they change the answer — so the
/// panel says what *this* route was planned with rather than what the
/// settings happen to hold now.
///
/// [`None`] for anything that was not planned: a proven route is searched
/// system by system, and an unaided one has no boost stars to plan over.
/// See [`Filter::tune`].
///
/// **The gap width is not said, because it is not one number.** The plan
/// climbs it — a chain that does not close on the goal is tried again a
/// jump wider ([`galos_route::highway::Highway::plan`]) — so
/// [`Tuning::reach`] is where the climb started rather than what the plan
/// used, and a line that quoted it would be describing a rung the answer
/// may not have come from.
///
/// The plan's own ask is said in its three forms, because they are three
/// different answers: a plan leaned by a percent, an exact plan that was
/// *tried* inside [`Tuning::allowance`] and may have ended up leaned
/// anyway, and an exact plan that was paid for and therefore is one. The
/// middle one says "where it was cheap" rather than claiming the chain it
/// may not have got — which is what this line could not do while the two
/// exact asks were one setting.
fn planned_with(filter: &Filter) -> Option<String> {
    let tune = filter.tune()?;
    let crossing = match tune.crossing {
        Crossing::Stepped => "stepped",
        Crossing::Searched => "searched",
    };
    let plan = match (tune.planning, tune.allowance) {
        (0, None) => "an exact plan".to_owned(),
        (0, Some(_)) => "an exact plan where it was cheap".to_owned(),
        (over, _) => format!("a plan leaned {over}%"),
    };
    Some(format!("{plan}, gaps {crossing}"))
}

/// How long the search that answered a filter took, for its panel
///
/// A trip is not a row of its own — it is the route its legs come to, joined
/// by [`crate::ui::bar::applied::as_one`] — so its time is its legs', and the longest of
/// them rather than the sum: the legs are searched at once, so the wait is
/// the slowest of them.
pub(super) fn took(
    filter: &Filter,
    legs: &[Filter],
    filters: &Filters,
) -> Option<Duration> {
    if let Some(took) = filters.took_of(filter) {
        return Some(took);
    }
    legs.iter().filter_map(|leg| filters.took_of(leg)).max()
}

/// The systems a filter admits, and the one the user picks out of them
///
/// Every system the database has for the filter, not only the ones the map
/// has fetched, since where a faction is, is most of what is being asked.
///
/// Answered the way the map itself is: one click says which system is meant
/// and a second says to go there, so a system reached through a list and a
/// system reached by its star are reached the same way.
///
/// Each line ends in a distance, and which distance it is follows from what
/// the list is: the jump that reaches the system where the filter is flown,
/// and how far off it is from the camera where it is not.
#[allow(clippy::too_many_arguments)]
pub(super) fn admitted(
    ui: &mut Ui,
    filter: &Filter,
    legs: &[Filter],
    systems: Option<&[System]>,
    took: Option<Duration>,
    // How many of a trip's legs are still being searched; see `summary`.
    plotting: usize,
    // And what became of this row's own search, where it has not landed a
    // route; see [`crate::map::filter::Plotted`].
    plotted: Option<Plotted>,
    // Which stops can supercharge, for the class each line says.
    boosts: &galos_route::Boosts,
    // And what the arrival star of each listed system is, where it has been
    // looked up; see [`StarClasses`].
    classes: &StarClasses,
    center: Option<DVec3>,
    picked: &mut Option<(System, bool)>,
    picked_stops: &mut Option<(Vec<System>, bool)>,
    described: &mut Option<System>,
    moved: &mut Option<MoveCamera>,
    worked: &mut Option<Filter>,
) {
    let Some(systems) = systems else {
        ui.label(egui::RichText::new("Looking...").weak());
        return;
    };

    if systems.is_empty() {
        ui.label(egui::RichText::new("No systems on record").weak());
        return;
    }

    ui.label(
        egui::RichText::new(summary(
            filter,
            systems.len(),
            took,
            plotting,
            plotted,
        ))
        .weak(),
    );
    // And how it was planned, on its own line: the first says what the
    // route had to be, this says how the plan went about finding it.
    if let Some(planned) = planned_with(filter) {
        ui.label(egui::RichText::new(planned).weak());
    }

    // What each line has to say about where its system is, which is not the
    // same question in the two kinds of list.
    //
    // A route is flown, so what is worth knowing about a system on one is the
    // jump that reaches it: how far it is from the system before, which is
    // what a ship has to be able to make. Measured off the list, which for a
    // route is already in the order it is travelled. The first system is
    // where the flying starts and no jump reaches it, so its line says
    // nothing.
    //
    // Everywhere else the systems are a set, in no order but the one this
    // list puts them in, and how far off they are from where the camera is
    // looking is both what orders them and what says why.
    let mut order: Vec<(&System, Option<f64>)> = if filter.ordered() {
        let mut legs = Vec::with_capacity(systems.len());
        let mut left = None;
        for system in systems {
            let at = DVec3::from(system.position);
            legs.push((system, left.map(|from: DVec3| from.distance(at))));
            left = Some(at);
        }
        legs
    } else {
        systems
            .iter()
            .map(|system| {
                (
                    system,
                    center.map(|at| at.distance(DVec3::from(system.position))),
                )
            })
            .collect()
    };

    // A filter with an order of its own is left in it. A route is travelled
    // from one end to the other, and a list of its systems put in any other
    // order is no longer a route, whatever it is sorted by.
    //
    // Where there is no such order, nearest first, from where the camera is
    // looking, which is the distance the whole map is measured in. Ordered
    // afresh each frame rather than once when the list arrived, so it goes on
    // answering which of these is near me as the user flies. That does mean
    // it reorders while the camera is moving; it holds still the moment it
    // stops, and the camera only moves when it is asked to.
    //
    // A stable sort, so that with no camera to measure from the order the
    // list arrived in is what is left.
    if !filter.ordered() {
        order.sort_by(|(_, one), (_, other)| match (one, other) {
            (Some(one), Some(other)) => one.total_cmp(other),
            _ => std::cmp::Ordering::Equal,
        });
    }

    // What flying it comes to, for a route. Under the summary, which says
    // what the ship was plotted at, this says what the plot asks of it: how
    // far it is all told, and the longest single jump, which is the one
    // deciding whether the ship as it stands can make the trip at all.
    //
    // Distances, and no fuel figure. Fuel is what a reader actually wants
    // to know and the map is not told the ship it would take to say it — so
    // the rule for working it out is on hover instead, exact for whatever
    // drive is fitted, where a figure of the map's own would have been a
    // third out for most of them. See [`fuel_rule`].
    if filter.ordered()
        && let Some((total, longest)) =
            flying(order.iter().filter_map(|(_, leg)| *leg))
    {
        ui.label(
            egui::RichText::new(format!(
                "{total:.1} Ly flown, longest jump {longest:.1} Ly"
            ))
            .weak(),
        )
        .on_hover_text(fuel_rule());
    }

    // And what it asks of the tank between refuellings. Under the distance,
    // because it is the other thing a plotted route can be impossible for: a
    // stretch with nothing to scoop is flown on the fuel the ship set out
    // with, and a stretch too far to cross on one tank strands it. Said only
    // where there is something to say. See [`Scooping`].
    if filter.ordered()
        && let Some(said) = Scooping::of(order.iter().map(|(system, leg)| {
            (system.name.as_str(), classes.of(system.address), *leg)
        }))
        .said()
    {
        ui.label(egui::RichText::new(said).weak()).on_hover_text(fuel_rule());
    }

    ui.add_space(MARGIN);

    // The list as text, for wherever it is wanted next. A route is flown in
    // the game with a hand on the keyboard, and a faction's holdings are read
    // against a spreadsheet, and neither is done off a window that cannot be
    // selected from. In the order the list is drawn in, and saying what each
    // line says, so what is copied is what is read.
    ui.horizontal(|ui| {
        if ui
            .button(if filter.ordered() { "Copy Route" } else { "Copy List" })
            .clicked()
        {
            ui.ctx().copy_text(as_text(&order, legs));
        }

        // Where the camera has to stand to see the whole of what the panel
        // lists, off the systems it is listing. Said as a button because the
        // gesture that asks it of a row cannot be asked of a window: a double
        // click on a panel's title rolls it up into its title bar, egui's
        // own reading of it, so a panel framed that way would fold shut on
        // the way.
        let framing =
            if filter.ordered() { "Frame Route" } else { "Frame List" };
        if ui.button(framing).clicked() {
            let places: Vec<DVec3> = order
                .iter()
                .map(|(system, _)| DVec3::from(system.position))
                .collect();
            if let Some((middle, extent)) =
                crate::map::route::spawn::framing(&places)
                && extent > 0.
            {
                *moved = Some(MoveCamera {
                    position: Some(middle),
                    framing: Some(extent),
                });
            }
        }
    });
    ui.add_space(MARGIN);

    // The list is not scrolled here. The panel it stands in is one scroll
    // area of its own (see [`panels`]), and a list scrolled inside a scrolled
    // panel is two bars to reach one row with. It was capped at eight lines
    // for the tiling's sake — a panel that ran the height of the viewport
    // left nowhere to put the next one — and the panel scrolling is what
    // answers that instead, so a faction's whole holdings are listed and the
    // one bar carries them.
    {
        // One row, wherever in the list it stands. `at` keys it by place
        // rather than by which system stands there: the list is put in order
        // afresh every frame, so a row holds its rectangle while the system
        // in it changes as the camera moves, which is the one thing egui
        // reads as a widget taking another's state.
        // Answers rather than acts, so that what a row asked for is written
        // where the row is drawn. Acting here would hold the borrow of it for
        // as long as the list, and the name over each leg has answers of its
        // own to write.
        // What stands after one stop's name: how far the jump onto it was,
        // where it is what sorts the list — a list in an order nobody can
        // see reads as an order nobody chose — and what kind of star waits
        // there.
        //
        // The real class where the index has been asked and answered, and
        // the one thing published for every system until then: a stop that
        // can supercharge, which is why the route came this way. Nothing at
        // all for a system nothing has scanned and no cone on, rather than a
        // guess at a spectrum.
        let reading = |system: &System, away: Option<f64>| {
            let class =
                classes.of(system.address).map(str::to_owned).or_else(|| {
                    boosts
                        .get(system.address)
                        .map(|boost| boost.named().to_owned())
                });
            match (class, away) {
                (Some(class), Some(away)) => {
                    Some(format!("{class}, {away:.1} Ly"))
                }
                (Some(class), None) => Some(class),
                (None, away) => away.map(|away| format!("{away:.1} Ly")),
            }
        };

        // Where those readings go, settled for the whole route before a line
        // of it is drawn: a stop whose class and distance have to go under
        // its name in a list where the next stop's fit beside it reads as
        // two kinds of row. See [`crate::ui::list::Rows`]. Said once and kept, the
        // lines being drawn from the same strings that were measured.
        let said: Vec<Option<String>> =
            order.iter().map(|(system, away)| reading(system, *away)).collect();
        let rows = crate::ui::list::Rows::of(
            ui,
            // A trip's stops are drawn indented under their leg's name, so
            // they get that much less than the panel has. A route with no
            // legs is drawn flush and gets the whole of it.
            ui.available_width()
                - if legs.is_empty() { 0. } else { ui.spacing().indent },
            order.iter().zip(&said).map(|((system, _), reading)| {
                (system.name.as_str(), reading.as_deref())
            }),
        );

        // One row, and what a click on it asked for.
        let line_of = |ui: &mut Ui, at: usize, system: &System| {
            crate::ui::list::system_line(
                ui,
                &system.name,
                said[at].clone(),
                rows,
                ("admitted", at),
            )
        };

        // A trip is listed leg by leg. One run of forty systems says nothing
        // about which of them the user asked for, so each leg is named and
        // its stops drawn in under it -- the same reading the bar's rows
        // give, in the panel that is about the whole of it.
        //
        // The grouping is `by_leg`'s, which is also what the copying reads,
        // so what is drawn and what is taken away are the one list.
        let mut at = 0;
        for (leg, stops) in by_leg(&order, legs) {
            // A leg's name is the control for the leg, read the way its row
            // in the bar is read: a click says it is the one being worked
            // with, and a double says to see the whole of it. The stops under
            // it are systems and answer as the lines of any other list do, so
            // a trip's panel offers what the bar and the search list offer
            // rather than being the one place a route cannot be reached from.
            if let Some(leg) = leg {
                // Named, and how many jumps it is flown in beside the name:
                // the same two readings its row in the bar gives, said by
                // the same [`crate::ui::bar::applied::hops_said`] so a leg cannot read
                // one way in the bar and another in the panel about it.
                let heading = ui
                    .horizontal(|ui| {
                        let named = ui.add(
                            egui::Label::new(
                                egui::RichText::new(leg.name()).strong(),
                            )
                            .selectable(false)
                            .sense(egui::Sense::click()),
                        );
                        if let Some(hops) = leg.hops() {
                            ui.label(
                                egui::RichText::new(
                                    crate::ui::bar::applied::hops_said(hops),
                                )
                                .weak(),
                            );
                        }
                        named
                    })
                    .inner;
                let settled = crate::ui::list::settled_click(
                    ui,
                    heading.id,
                    heading.clicked(),
                    heading.double_clicked(),
                );
                match crate::ui::list::asked_of_row(
                    false,
                    false,
                    false,
                    false,
                    heading.double_clicked(),
                    settled.is_some(),
                ) {
                    // Every stop the leg runs through, taken off the whole
                    // list rather than off the rows drawn under the name.
                    // The two differ by one: a leg sets out from the system
                    // the leg before it landed on, which is drawn up there
                    // and is still where this one starts. Framed without it
                    // the camera stands over the leg's tail, and the bar's
                    // own row for the same leg would frame it differently.
                    Some(crate::ui::list::RowGesture::Frame) => {
                        let places: Vec<DVec3> = order
                            .iter()
                            .filter(|(system, _)| {
                                leg.place_of(system.address).is_some()
                            })
                            .map(|(system, _)| DVec3::from(system.position))
                            .collect();
                        if let Some((middle, extent)) =
                            crate::map::route::spawn::framing(&places)
                            && extent > 0.
                        {
                            *moved = Some(MoveCamera {
                                position: Some(middle),
                                framing: Some(extent),
                            });
                        }
                    }
                    // The same two things a click on the leg's row in the
                    // bar means, and for the same reason: the leg is what is
                    // being worked with, and what it was plotted between is
                    // what the user is holding when they reach for its name.
                    // Both answers are handed back rather than acted on here,
                    // and the selection is picked out through
                    // [`crate::map::selection::Selection::pick_out`], so
                    // a leg reached through the bar and the same leg reached
                    // through this panel cannot come to two different things.
                    Some(crate::ui::list::RowGesture::Select) => {
                        let ends = leg.stops();
                        *picked_stops = Some((
                            order
                                .iter()
                                .filter(|(system, _)| {
                                    ends.contains(&system.address)
                                })
                                .map(|(system, _)| (*system).clone())
                                .collect(),
                            crate::ui::list::gathering_with(
                                settled.unwrap_or_default(),
                            ),
                        ));
                        *worked = Some(leg.clone());
                    }
                    _ => {}
                }
                heading.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            let mut rows = |ui: &mut Ui| {
                for (step, (system, _)) in stops.iter().enumerate() {
                    match line_of(ui, at + step, system) {
                        Some(SystemAction::Select { gathering }) => {
                            *picked = Some(((*system).clone(), gathering))
                        }
                        Some(SystemAction::Travel) => {
                            *moved = Some(MoveCamera {
                                position: Some(DVec3::from(system.position)),
                                framing: None,
                            })
                        }
                        Some(SystemAction::Describe) => {
                            *described = Some((*system).clone())
                        }
                        None => {}
                    }
                }
            };
            match leg {
                Some(leg) => {
                    ui.indent(("leg", leg.name()), |ui| rows(ui));
                }
                None => rows(ui),
            }
            at += stops.len();
        }
    }
}

/// What a route comes to, flown: how far all told, and the longest jump in it
///
/// Nothing for a route with no leg to fly, there being nothing to add up and
/// no jump to be the longest. One pass, since both answers come off the same
/// legs and a route is walked to draw it anyway.
fn flying(legs: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    legs.fold(None, |so_far, leg| match so_far {
        None => Some((leg, leg)),
        Some((total, longest)) => Some((total + leg, longest.max(leg))),
    })
}

/// How a trip's list breaks into its legs
///
/// Each leg's name and the stops that fall under it, in the order flown. One
/// group named for nothing where there are no legs, which is every filter but
/// a trip: a list that is not a trip's is one list.
///
/// The stop a leg lands on is the stop the next sets out from and stands in
/// the joined list once, so every leg after the first begins on the one
/// before's last.
///
/// Here rather than in the drawing because the copying wants it too, and a
/// list drawn in one grouping and copied in another would be two answers to
/// the one question.
fn by_leg<'a, 'l>(
    order: &'a [(&'a System, Option<f64>)],
    legs: &'l [Filter],
) -> Vec<(Option<&'l Filter>, &'a [(&'a System, Option<f64>)])> {
    if legs.is_empty() {
        return vec![(None, order)];
    }

    let mut groups = Vec::with_capacity(legs.len());
    let mut at = 0;
    for leg in legs {
        let Filter::Route { systems: hops, .. } = leg else { continue };
        let takes = hops.len().saturating_sub(usize::from(at > 0));
        let Some(stops) = order.get(at..at + takes) else { break };
        groups.push((Some(leg), stops));
        at += takes;
    }
    groups
}

/// A list of systems as text, one to a line, as the panel draws them
///
/// Each line is the name and, where the list gives one, the distance the
/// panel's line ends in: the jump that reaches a system on a route, and how
/// far off it is from the camera otherwise. Set apart by a tab, so what is
/// pasted into a spreadsheet lands in two columns and what is pasted anywhere
/// else still reads as one line about one system.
fn as_text(order: &[(&System, Option<f64>)], legs: &[Filter]) -> String {
    let mut said: Vec<String> = Vec::new();
    for (named, stops) in by_leg(order, legs) {
        // A trip's legs are named and their stops drawn in under them, so the
        // text says the same. Two spaces rather than a tab, the tab already
        // standing between a name and the distance after it.
        let indent = if named.is_some() { "  " } else { "" };
        if let Some(leg) = named {
            said.push(leg.name().to_owned());
        }
        for (system, away) in stops {
            said.push(match away {
                Some(away) => {
                    format!("{indent}{}\t{away:.2} Ly", system.name)
                }
                None => format!("{indent}{}", system.name),
            });
        }
    }

    said.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::galaxy::tests::system;
    use crate::testing::{context, painted, words};
    use galos_route::graph::{Drive, Routing, Tuning};

    use crate::ui::testing::faction;

    /// And a filter's list of systems, which carries a distance and a mark
    #[test]
    fn a_system_list_paints_in_a_color() {
        let systems = [system(1), system(2)];
        painted(|ui| {
            admitted(
                ui,
                &faction(7),
                &[],
                Some(&systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });
    }

    /// So does one a route lists in the order it is travelled
    #[test]
    fn a_route_list_paints_in_a_color() {
        let systems = [system(1), system(2)];
        let route = Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        painted(|ui| {
            admitted(
                ui,
                &route,
                &[],
                Some(&systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });
    }

    /// A system at `place`, otherwise as bare as [`system`]
    fn placed(address: i64, place: [f64; 3]) -> System {
        let mut system = system(address);
        system.position = place;
        system
    }

    /// Every distance the list puts on the lines of a route through `places`
    ///
    /// Read off what was painted rather than asked of the row, a row laying
    /// its own text out having no label to be asked what it says.
    ///
    /// The camera is a hundred light years off, so a distance measured from
    /// it could not be read as a jump.
    fn flown(places: &[[f64; 3]]) -> Vec<String> {
        let systems: Vec<System> = places
            .iter()
            .enumerate()
            .map(|(place, at)| placed(place as i64 + 1, *at))
            .collect();
        let route = Filter::Route {
            label: "A -> B".to_owned(),
            systems: (1..=places.len() as i64).collect(),
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };

        crate::testing::words(|ui| {
            admitted(
                ui,
                &route,
                &[],
                Some(&systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::new(100., 0., 0.)),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        })
        .into_iter()
        // A line that is a distance and nothing else. What the whole route
        // comes to is said in light years too, and is not a row's.
        .filter(|said| {
            said.strip_suffix(" Ly")
                .is_some_and(|figure| figure.parse::<f64>().is_ok())
        })
        .collect()
    }

    /// A route says how far the jump that reaches each system is
    ///
    /// It is flown, so what is worth knowing about a system on one is whether
    /// the ship can get to it from the one before. How far it is from the
    /// camera answers a question nobody asked of a route.
    ///
    /// Two distances over three systems, the first being where the flying
    /// starts: no jump reaches it, so its line has nothing to say.
    #[test]
    fn a_route_says_how_far_each_jump_is() {
        let said = flown(&[[0., 0., 0.], [3., 4., 0.], [3., 4., 12.]]);

        assert_eq!(said, vec!["5.0 Ly", "12.0 Ly"], "{said:?}");
    }

    /// A set of systems says how far off each one is instead
    ///
    /// Nothing about a faction's holdings is a sequence, so there is no jump
    /// to measure and the distance the whole map is read in is what is left.
    #[test]
    fn a_set_of_systems_says_how_far_off_each_is() {
        let systems = [placed(1, [3., 4., 0.]), placed(2, [0., 0., 12.])];

        let said = crate::testing::words(|ui| {
            admitted(
                ui,
                &faction(7),
                &[],
                Some(&systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });

        assert!(said.contains(&"5.0 Ly".to_owned()), "{said:?}");
        assert!(said.contains(&"12.0 Ly".to_owned()), "{said:?}");
    }

    /// A route between `label`'s ends, plotted for a ship reaching `range`
    /// Proven rather than whatever the form opens at: the summary tests
    /// below are about what a route *says* it was plotted for, so they say
    /// what it was plotted for rather than reading the default and
    /// following it wherever it moves.
    fn plotted_for(label: &str, range: &str) -> Filter {
        plotted_with(label, range, Drive::Unaided, Routing::FEWEST)
    }

    /// A leg of a trip, named, through the stops it runs
    fn route_through(label: &str, stops: &[i64]) -> Filter {
        Filter::Route {
            label: label.to_owned(),
            systems: stops.to_vec(),
            range: "50".to_owned(),
            trip: Some("SOL -> LAVE -> DISO".to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        }
    }

    /// The same, for a named drive and search mode
    fn plotted_with(
        label: &str,
        range: &str,
        drive: Drive,
        how: Routing,
    ) -> Filter {
        Filter::Route {
            label: label.to_owned(),
            systems: vec![1, 2],
            range: range.to_owned(),
            trip: None,
            drive,
            how,
            tune: Tuning::default(),
        }
    }

    /// A route's panel says the whole of what it was plotted for
    ///
    /// A panel is titled with what its filter is called and a route is called
    /// after its two ends, so this line is the only thing on screen telling
    /// two plots between the same pair apart — and each of the three can
    /// change which systems came back.
    ///
    /// A range and not a jump. What the ship can cross in one is what the
    /// route was worked out against; what it actually crosses is on the lines
    /// below, and is usually less — unless a jet cone was allowed, which is
    /// why the drive is said beside it: a nine hundred light year jump under
    /// a hundred and fifty light year range is a supercharge and not a bug,
    /// and nothing else on screen says one was permitted.
    #[test]
    fn a_route_panel_says_what_it_was_plotted_for() {
        assert_eq!(
            summary(&plotted_for("SOL -> BARNARD", "10"), 12, None, 0, None),
            "12 systems, 10 Ly range, optimal, fewest jumps"
        );
        assert_eq!(
            summary(
                &plotted_with(
                    "SOL -> COLONIA",
                    "150",
                    Drive::Optimised,
                    Routing::QUICK
                ),
                116,
                None,
                0,
                None,
            ),
            "116 systems, 150 Ly range, SCO supercharged, 95% optimality, \
             fewest jumps, the nearest 512 expanded"
        );
    }

    /// And what the search cost, where this session is the one that paid it
    ///
    /// The other half of what a search mode means: `quick` and `direct`
    /// differ by a jump or two and by minutes, so the row that says which
    /// was asked should say what it came to. Nothing at all for a route
    /// this session did not plot, rather than a zero.
    #[test]
    fn a_route_panel_says_what_the_search_cost() {
        let said = summary(
            &plotted_for("SOL -> BARNARD", "10"),
            12,
            Some(Duration::from_millis(2230)),
            0,
            None,
        );
        assert!(said.ends_with("plotted in 2.2 s"), "{said}");

        let untimed =
            summary(&plotted_for("SOL -> BARNARD", "10"), 12, None, 0, None);
        assert!(!untimed.contains("plotted"), "{untimed}");
    }

    /// A stop that can supercharge says so, beside its jump
    ///
    /// Which is why the route came that way, and the jump *out* of it is the
    /// long blue one on the line. The only class the index publishes for
    /// every system is what it can supercharge on, so a stop with no cone
    /// says nothing about its star rather than guessing at a spectrum.
    #[test]
    fn a_stop_that_can_supercharge_says_so() {
        use galos_index::Boost;
        use galos_index::records::SystemBoost;

        let route = route_through("SOL -> LAVE", &[1, 2, 3]);
        let systems: Vec<System> = (1..=3).map(system).collect();
        let boosts = galos_route::Boosts::of(vec![SystemBoost {
            address: 2,
            boost: Boost::Neutron,
            position: [0., 0., 0.],
        }]);

        let said = crate::testing::words(|ui| {
            admitted(
                ui,
                &route,
                &[],
                Some(&systems),
                None,
                0,
                None,
                &boosts,
                &StarClasses::default(),
                None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            )
        });

        assert!(
            said.iter().any(|line| line.contains("neutron star")),
            "the cone on the route was not named: {said:?}"
        );
        assert_eq!(
            said.iter().filter(|line| line.contains("neutron star")).count(),
            1,
            "a system with no cone was called one: {said:?}",
        );
    }

    /// A leg's heading says how many jumps it is, as its row does
    ///
    /// The panel about a trip is about the same legs the bar has rows for,
    /// so a leg had better not read one way in the one and another in the
    /// other. Said by [`crate::ui::bar::applied::hops_said`] in both places rather than
    /// formatted twice.
    #[test]
    fn a_legs_heading_says_its_hops() {
        let trip = plotted_for("3 Leg Route", "50");
        let legs = [
            route_through("SOL -> LAVE", &[1, 2, 3]),
            route_through("LAVE -> DISO", &[3, 4]),
        ];
        let systems: Vec<System> = (1..=4).map(system).collect();

        let said = crate::testing::words(|ui| {
            admitted(
                ui,
                &trip,
                &legs,
                Some(&systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            )
        });

        // Two jumps in the first leg, one in the second, said the way the
        // bar says them.
        assert!(said.iter().any(|line| line == "2 hops"), "{said:?}");
        assert!(said.iter().any(|line| line == "1 hop"), "{said:?}");
        assert_eq!(
            crate::ui::bar::applied::hops_said(1),
            "1 hop",
            "the bar and the panel disagree about one jump",
        );
    }

    /// A trip still being plotted says so, rather than describing part of
    /// itself as the whole
    ///
    /// The reported trouble: a panel opened before the last leg lands reads
    /// as a finished trip. Everything under the summary — the systems, the
    /// distance flown, the longest jump — is about the legs that *have*
    /// landed, and a route through some of the stops is not the route the
    /// panel is titled after.
    #[test]
    fn a_trip_still_being_plotted_says_what_is_missing() {
        let trip = plotted_for("3 Leg Route", "50");

        let waiting = summary(&trip, 120, None, 1, None);
        assert!(
            waiting.ends_with("1 leg still being plotted"),
            "nothing said a leg was missing: {waiting}"
        );
        let two = summary(&trip, 120, None, 2, None);
        assert!(two.ends_with("2 legs still being plotted"), "{two}");

        // And once they are all in, it says only what it is.
        let whole = summary(&trip, 168, None, 0, None);
        assert!(
            !whole.contains("still being plotted"),
            "a finished trip said it was waiting: {whole}"
        );
    }

    /// And a leg's own panel says what became of its search
    ///
    /// A leg's row stands from the ask onward, so a panel can be opened over
    /// one that has no route yet: what it lists is then the two ends it was
    /// asked between, and a summary reading "2 systems, 50 Ly range" over
    /// them claims a route nobody has found.
    #[test]
    fn a_leg_with_no_route_says_so_rather_than_listing_two_systems() {
        let leg = plotted_for("SOL -> COLONIA", "50");

        let searching = summary(&leg, 2, None, 0, Some(Plotted::Searching));
        assert!(searching.ends_with("— searching"), "{searching}");
        let stopped = summary(&leg, 2, None, 0, Some(Plotted::Stopped));
        assert!(stopped.ends_with("— stopped"), "{stopped}");
        let unflown = summary(&leg, 2, None, 0, Some(Plotted::Unreachable));
        assert!(unflown.ends_with("— no route"), "{unflown}");

        // And a route that landed says what it came to and nothing else.
        let landed = summary(&leg, 168, None, 0, Some(Plotted::Landed));
        assert!(!landed.contains('—'), "{landed}");
    }

    /// And how it was planned, where it was planned at all
    ///
    /// What was *asked* is on the panel: how hard the plan was worked and
    /// how its gaps were crossed, both of them part of a route's identity
    /// because they change the answer. The gap width is not among them —
    /// the plan climbs it, so there is no one number to quote.
    ///
    /// Nothing for a route nothing planned — a proven route is searched
    /// system by system, and an unaided ship has no boost stars — where a
    /// line about the plan would be describing machinery that never ran.
    #[test]
    fn a_route_panel_says_how_it_was_planned() {
        let asked = |drive: Drive, how: Routing, tune: Tuning| Filter::Route {
            label: "SOL -> COLONIA".to_owned(),
            systems: vec![1, 2],
            range: "50".to_owned(),
            trip: None,
            drive,
            how,
            tune,
        };
        let wide = Tuning { reach: 450, ..Tuning::default() };

        // The exact ask in both its forms, because they are two answers:
        // one tried inside the allowance and one paid for.
        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, wide))
                .as_deref(),
            Some("an exact plan where it was cheap, gaps stepped"),
        );
        let paid = Tuning { allowance: None, ..wide };
        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, paid))
                .as_deref(),
            Some("an exact plan, gaps stepped"),
        );

        // And the plan's own leaning, which is its own setting rather than
        // the route's percent: a reader who leaned the plan has a
        // different route and the panel is what says so.
        let leaned = Tuning { planning: 20, ..wide };
        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, leaned))
                .as_deref(),
            Some("a plan leaned 20%, gaps stepped"),
        );

        // And the reach is not quoted, the plan having climbed it.
        assert!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, wide))
                .is_some_and(|said| !said.contains("450")),
            "the panel quoted a rung the answer may not have come from",
        );

        let searched = Tuning { crossing: Crossing::Searched, ..wide };
        assert!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, searched))
                .is_some_and(|said| said.ends_with("searched")),
            "the crossing was not said",
        );

        // Nothing planned it: proven, and unaided.
        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::FEWEST, wide)),
            None,
            "a proven route was said to be planned",
        );
        assert_eq!(
            planned_with(&asked(Drive::Unaided, Routing::QUICK, wide)),
            None,
            "an unaided route was said to be planned",
        );
    }

    /// Two plots between the same ends are told apart by it
    ///
    /// Which is the whole of why it is said. Both panels are titled the same,
    /// both list systems between the same two, and what the user asked for is
    /// the difference between them.
    #[test]
    fn two_routes_between_the_same_ends_read_apart() {
        let near =
            summary(&plotted_for("SOL -> BARNARD", "10"), 12, None, 0, None);
        let far =
            summary(&plotted_for("SOL -> BARNARD", "20"), 7, None, 0, None);

        assert_ne!(near, far);
        assert!(near.contains("10 Ly"), "{near}");
        assert!(far.contains("20 Ly"), "{far}");
    }

    /// A filter that was never plotted says how many and no more
    ///
    /// A faction and a hand-picked set were never asked a range, and a panel
    /// that answered one for them would be answering for the user.
    #[test]
    fn a_filter_that_was_not_plotted_says_only_how_many() {
        assert_eq!(summary(&faction(7), 12, None, 0, None), "12 systems");
        assert_eq!(
            summary(
                &Filter::Systems {
                    label: "3 systems".to_owned(),
                    systems: vec![1, 2, 3],
                },
                3,
                None,
                0,
                None,
            ),
            "3 systems"
        );
    }

    /// And the panel draws whatever the summary came to
    ///
    /// Read off what was painted, the line being a label the panel lays out
    /// from what `summary` answered.
    #[test]
    fn the_panel_draws_the_summary() {
        let systems = [placed(1, [0., 0., 0.]), placed(2, [3., 4., 0.])];

        let said = crate::testing::words(|ui| {
            admitted(
                ui,
                &plotted_for("SOL -> BARNARD", "10"),
                &[],
                Some(&systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });

        assert!(
            said.contains(
                &"2 systems, 10 Ly range, optimal, fewest jumps".to_owned()
            ),
            "{said:?}"
        );
    }

    /// What a route comes to, flown, is the whole of it and its worst leg
    ///
    /// The longest jump is what says whether the ship can make the trip; the
    /// range it was plotted at was what was asked, not what is needed. A
    /// route with no leg to fly has neither answer.
    #[test]
    fn a_route_comes_to_its_whole_length_and_its_longest_jump() {
        assert_eq!(flying([5., 12., 3.].into_iter()), Some((20., 12.)));
        assert_eq!(flying([7.].into_iter()), Some((7., 7.)));
        assert_eq!(flying(std::iter::empty()), None);
    }

    /// And the panel says it for a route alone
    ///
    /// A faction's holdings are not flown in any order, so there is nothing
    /// about them to add up.
    ///
    /// Distances, and no fuel figure: the map is not told the ship a fuel
    /// figure would take, so the rule for working one out is on hover
    /// instead. See [`fuel_rule`].
    #[test]
    fn the_panel_says_what_a_route_comes_to_and_no_more() {
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [3., 4., 0.]),
            placed(3, [3., 4., 12.]),
        ];

        let route = listing(&plotted_for("SOL -> BARNARD", "10"), &held);
        assert!(
            route.contains(&"17.0 Ly flown, longest jump 12.0 Ly".to_owned()),
            "{route:?}"
        );

        let holdings = listing(&faction(7), &held);
        assert!(
            !holdings.iter().any(|said| said.contains("flown")),
            "{holdings:?}"
        );
    }

    /// A trip's panel lists it leg by leg, each leg's stops under its name
    ///
    /// One run of forty systems says nothing about which of them the user
    /// asked for. The figures above are still the whole trip's, and the seam
    /// stands in one leg only: the stop a leg lands on is where the next sets
    /// out from, and naming it twice would count a jump from a system to
    /// itself.
    #[test]
    fn a_trips_panel_lists_it_leg_by_leg() {
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [17., 0., 0.]),
            placed(4, [20., 0., 0.]),
            placed(5, [26., 0., 0.]),
        ];
        let leg = |label: &str, systems: Vec<i64>| Filter::Route {
            label: label.to_owned(),
            systems,
            range: "12".to_owned(),
            trip: Some("A -> C -> E".to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let legs = vec![
            leg("FIRST LEG", vec![1, 2, 3]),
            leg("SECOND LEG", vec![3, 4, 5]),
        ];

        let said = crate::testing::words(|ui| {
            admitted(
                ui,
                &leg("2 Leg Route", vec![1, 2, 3, 4, 5]),
                &legs,
                Some(&held),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });
        let at = |what: &str| {
            said.iter()
                .position(|line| line.starts_with(what))
                .unwrap_or_else(|| panic!("{what} was painted: {said:?}"))
        };

        // The whole trip's figures, above either leg. Matched on the start
        // of the line, the fuel beside the distance being
        // [`the_panel_says_what_a_route_comes_to_and_no_more`]'s business.
        assert!(at("26.0 Ly flown, longest jump 12.0 Ly") < at("FIRST LEG"));
        // Each leg's stops under its own name, in the order flown.
        assert!(at("FIRST LEG") < at("TEST 1"));
        assert!(at("TEST 3") < at("SECOND LEG"));
        assert!(at("SECOND LEG") < at("TEST 4"));
        // And the seam once: the first leg lands on Test 3, the second sets
        // out from it.
        assert_eq!(said.iter().filter(|line| *line == "TEST 3").count(), 1);
    }

    /// The list copied is the list drawn, a system to a line
    ///
    /// With the distance each line ends in, where it ends in one, set off by
    /// a tab so a paste lands in two columns. The first system of a route is
    /// reached by no jump, and its line says nothing after the name.
    #[test]
    fn the_list_is_copied_as_it_is_drawn() {
        let (sol, wolf) = (
            crate::map::galaxy::tests::named(1, "SOL"),
            crate::map::galaxy::tests::named(2, "WOLF 359"),
        );
        let order = [(&sol, None), (&wolf, Some(7.78))];

        assert_eq!(as_text(&order, &[]), "SOL\nWOLF 359\t7.78 Ly");
    }

    /// What a filter's panel offers to copy is named for what the list is
    ///
    /// A route is a way to fly and a faction's holdings are a set of places,
    /// and the button that hands either to the clipboard should say which it
    /// is handing over.
    #[test]
    fn a_route_offers_its_route_and_a_set_offers_its_list() {
        let held = [crate::map::galaxy::tests::named(1, "SOL")];

        let route = listing(&plotted_for("SOL -> BARNARD", "10"), &held);
        assert!(route.contains(&"Copy Route".to_owned()), "{route:?}");
        assert!(!route.contains(&"Copy List".to_owned()), "{route:?}");

        let holdings = listing(&faction(7), &held);
        assert!(holdings.contains(&"Copy List".to_owned()), "{holdings:?}");
        assert!(!holdings.contains(&"Copy Route".to_owned()), "{holdings:?}");
    }

    /// What a filter's list paints, line by line
    fn listing(filter: &Filter, systems: &[System]) -> Vec<String> {
        words(|ui| {
            admitted(
                ui,
                filter,
                &[],
                Some(systems),
                None,
                0,
                None,
                &galos_route::Boosts::absent(),
                &StarClasses::default(),
                None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        })
    }

    /// Where each piece of text landed, and what it said
    fn placed_text(
        ctx: &egui::Context,
        input: egui::RawInput,
        contents: impl FnMut(&mut Ui),
    ) -> Vec<(String, egui::Rect)> {
        let mut contents = contents;
        let output = ctx.run_ui(input, |ui| contents(ui));
        let mut found = Vec::new();
        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    text.galley.text().to_owned(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        for shape in &output.shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// A route's panel offers to frame it, since its title cannot be doubled
    ///
    /// The gesture that frames a filter's row is spoken for on a window:
    /// egui rolls a panel up into its title bar on a double click there. So
    /// the panel says it in words, beside the button that copies the same
    /// list, and asks the camera for the whole of what it lists.
    ///
    /// Driven at the position the button was painted at, the whole point
    /// being that there is something there to press.
    #[test]
    fn a_routes_panel_frames_it_by_a_button() {
        let route = Filter::Route {
            label: "SOL -> DISO".to_owned(),
            systems: vec![1, 2, 3],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [26., 0., 0.]),
        ];

        let ctx = context();
        let pass = |input: egui::RawInput, moved: &mut Option<MoveCamera>| {
            placed_text(&ctx, input, |ui| {
                admitted(
                    ui,
                    &route,
                    &[],
                    Some(&held),
                    None,
                    0,
                    None,
                    &galos_route::Boosts::absent(),
                    &StarClasses::default(),
                    Some(DVec3::ZERO),
                    &mut None,
                    &mut None,
                    &mut None,
                    moved,
                    &mut None,
                );
            })
        };

        // Twice, the first pass being where the button is placed.
        pass(egui::RawInput::default(), &mut None);
        let text = pass(egui::RawInput::default(), &mut None);
        let at = text
            .iter()
            .find(|(said, _)| said == "Frame Route")
            .expect("a button to frame the route")
            .1
            .center();

        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut moved = None;
        pass(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
                ..Default::default()
            },
            &mut moved,
        );

        // The middle of what it spans and the reach to the far end of it, as
        // the camera is stood back from a row's systems.
        let moved = moved.expect("the camera was asked to frame the route");
        assert_eq!(moved.position, Some(DVec3::new(13., 0., 0.)));
        assert_eq!(moved.framing, Some(13.));
    }

    /// A leg in a trip's panel is reached the way its row in the bar is
    ///
    /// A click on the name says the leg is the one being worked with, and a
    /// double says to see the whole of it. The stops under it are systems and
    /// answer as any list's lines do, so a route drawn as part of a trip can
    /// be got at from the panel about the trip rather than only from the bar.
    ///
    /// Driven at the position the name was actually painted at, since where
    /// the press lands is the whole of what is being claimed.
    #[test]
    fn a_leg_is_worked_with_by_a_click_and_framed_by_a_double() {
        let trip = Filter::Route {
            label: "SOL -> LAVE -> DISO".to_owned(),
            systems: vec![1, 2, 3, 4, 5],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let legs = vec![
            Filter::Route {
                label: "SOL -> LAVE".to_owned(),
                systems: vec![1, 2, 3],
                range: "10".to_owned(),
                trip: Some("SOL -> LAVE -> DISO".to_owned()),
                drive: Drive::Unaided,
                how: Routing::default(),
                tune: Tuning::default(),
            },
            Filter::Route {
                label: "LAVE -> DISO".to_owned(),
                systems: vec![3, 4, 5],
                range: "10".to_owned(),
                trip: Some("SOL -> LAVE -> DISO".to_owned()),
                drive: Drive::Unaided,
                how: Routing::default(),
                tune: Tuning::default(),
            },
        ];
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [17., 0., 0.]),
            placed(4, [20., 0., 0.]),
            placed(5, [26., 0., 0.]),
        ];

        let ctx = context();
        let pass = |input: egui::RawInput,
                    stops: &mut Option<(Vec<System>, bool)>,
                    moved: &mut Option<MoveCamera>,
                    worked: &mut Option<Filter>| {
            placed_text(&ctx, input, |ui| {
                admitted(
                    ui,
                    &trip,
                    &legs,
                    Some(&held),
                    None,
                    0,
                    None,
                    &galos_route::Boosts::absent(),
                    &StarClasses::default(),
                    Some(DVec3::ZERO),
                    &mut None,
                    stops,
                    &mut None,
                    moved,
                    worked,
                );
            })
        };

        pass(egui::RawInput::default(), &mut None, &mut None, &mut None);
        let text =
            pass(egui::RawInput::default(), &mut None, &mut None, &mut None);
        let at = text
            .iter()
            .find(|(said, _)| said == "LAVE -> DISO")
            .expect("the second leg's name")
            .1
            .center();

        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        // A click is answered once no double can still arrive
        // (`crate::ui::list::settled_click`), so the frames carry their own clock
        // and the gesture takes as many of them as a user's would.
        let frame = |time: f64, events| egui::RawInput {
            time: Some(time),
            events,
            ..Default::default()
        };
        // Longer than egui's `max_double_click_delay`, which is when a click
        // stops being half of anything.
        let settled = 0.4;

        let (mut stops, mut moved, mut worked) = (None, None, None);
        pass(
            frame(
                1.,
                vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
            ),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        assert!(worked.is_none(), "the click was answered before its window");
        pass(
            frame(1. + settled, Vec::new()),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        assert_eq!(worked.as_ref().map(|leg| leg.name()), Some("LAVE -> DISO"));
        assert!(moved.is_none(), "a click asked the camera for nothing");
        // And what the leg was plotted between, which is what the same click
        // on its row in the bar picks out: its two ends and none of the hops
        // it passes through on the way.
        let (picked, gathering) = stops.expect("the leg's stops");
        assert_eq!(
            picked.iter().map(|system| system.address).collect::<Vec<_>>(),
            vec![3, 5],
            "the hops came with it, or the ends did not"
        );
        assert!(!gathering, "a bare click was read as gathering");

        // The stops it runs through, which is one more than the rows drawn
        // under its name: it sets out from the system the leg before landed
        // on, and that row belongs to the leg before. Framed off the rows
        // alone this would read 23.0 and 3.0, standing the camera over the
        // leg's tail rather than over the leg.
        //
        // A press per frame, since that is what a double click is: egui
        // raises the click on the first release and the double on the second,
        // a frame apart, and the whole point is that the first does not act.
        let (mut stops, mut moved, mut worked) = (None, None, None);
        pass(
            frame(
                3.,
                vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
            ),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        pass(
            frame(3.05, vec![button(true), button(false)]),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        let framed =
            moved.as_ref().expect("a double asked the camera to frame it");
        assert_eq!(framed.position, Some(DVec3::new(21.5, 0., 0.)));
        assert_eq!(framed.framing, Some(4.5));
        assert!(worked.is_none(), "the double stood in for the click");
        assert!(stops.is_none(), "the double picked its stops out as well");

        // And nothing arrives afterwards: the double took the pending click
        // with it rather than leaving it to fire once the window passed.
        pass(
            frame(3. + settled, Vec::new()),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        assert!(worked.is_none(), "the double's first click landed late");
        assert!(stops.is_none(), "the double picked its stops out late");
    }

    /// And the modifier reads there as it does everywhere else
    ///
    /// A click means these systems, a click with the modifier means these as
    /// well: the leg's stops joining what was already picked out rather than
    /// replacing it. Read off the same press, since the modifier is part of
    /// the press and not a setting.
    #[test]
    fn a_leg_clicked_with_the_modifier_gathers_its_stops() {
        let trip = Filter::Route {
            label: "SOL -> LAVE".to_owned(),
            systems: vec![1, 2, 3],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let legs = vec![trip.clone()];
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [17., 0., 0.]),
        ];

        let ctx = context();
        let pass =
            |input: egui::RawInput, stops: &mut Option<(Vec<System>, bool)>| {
                placed_text(&ctx, input, |ui| {
                    admitted(
                        ui,
                        &trip,
                        &legs,
                        Some(&held),
                        None,
                        0,
                        None,
                        &galos_route::Boosts::absent(),
                        &StarClasses::default(),
                        Some(DVec3::ZERO),
                        &mut None,
                        stops,
                        &mut None,
                        &mut None,
                        &mut None,
                    );
                })
            };

        pass(egui::RawInput::default(), &mut None);
        let text = pass(egui::RawInput::default(), &mut None);
        let at = text
            .iter()
            .find(|(said, _)| said == "SOL -> LAVE")
            .expect("the leg's name")
            .1
            .center();

        let mut keys = egui::Modifiers::default();
        keys.shift = true;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: keys,
        };
        let mut stops = None;
        pass(
            egui::RawInput {
                time: Some(1.),
                events: vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
                modifiers: keys,
                ..Default::default()
            },
            &mut stops,
        );
        // Answered once no double can still arrive; see
        // [`crate::ui::list::settled_click`]. The modifier is read off the press
        // rather than off this frame, which is the whole of what is asked
        // here — it is not held down any more.
        pass(
            egui::RawInput { time: Some(1.4), ..Default::default() },
            &mut stops,
        );

        let (_, gathering) = stops.expect("the leg's stops");
        assert!(gathering, "the modifier was not read off the press");
    }
}
